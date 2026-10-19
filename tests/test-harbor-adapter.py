#!/usr/bin/env python3
"""Focused standard-library tests for the pinned Harbor adapter."""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tempfile
import tomllib
import types
import unittest
from types import SimpleNamespace
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))


class FakeNonZeroExit(RuntimeError):
    """Stand-in for Harbor's non-zero process exception."""


class FakeNetworkConnectionError(FakeNonZeroExit):
    """Stand-in for Harbor's transport-failure exception."""


class FakeUnknownApiError(FakeNonZeroExit):
    """Stand-in for Harbor's unclassified API-response exception."""


class FakeLogger:
    def __init__(self) -> None:
        self.errors: list[str] = []

    def error(self, message: str) -> None:
        self.errors.append(message)


class FakeProcess:
    def __init__(
        self,
        return_code: int = 0,
        *,
        stdout: str = "",
        stderr: str = "",
    ) -> None:
        self.return_code = return_code
        self.stdout = stdout
        self.stderr = stderr


class FakeAgentContext:
    def __init__(self) -> None:
        self.metadata = None
        self.n_input_tokens = None
        self.n_output_tokens = None


class FakeTrialConfig:
    @classmethod
    def model_validate_json(cls, value: str) -> SimpleNamespace:
        data = json.loads(value)
        agent = data.get("agent", {})
        return SimpleNamespace(
            agent=SimpleNamespace(
                override_timeout_sec=agent.get("override_timeout_sec"),
                max_timeout_sec=agent.get("max_timeout_sec"),
            ),
            timeout_multiplier=data.get("timeout_multiplier", 1.0),
            agent_timeout_multiplier=data.get("agent_timeout_multiplier"),
        )


class FakeTaskConfig:
    @classmethod
    def model_validate_toml(cls, value: str) -> SimpleNamespace:
        data = tomllib.loads(value)
        return SimpleNamespace(
            agent=SimpleNamespace(timeout_sec=data.get("agent", {}).get("timeout_sec")),
            steps=data.get("steps", []),
        )


class FakeBaseInstalledAgent:
    def __init__(
        self,
        *,
        logs_dir: Path,
        model_name: str,
        version: str,
        extra_env: dict[str, str],
        **kwargs: object,
    ) -> None:
        self.base_init = {
            "logs_dir": logs_dir,
            "model_name": model_name,
            "version": version,
            "extra_env": dict(extra_env),
            "kwargs": dict(kwargs),
        }
        self._extra_env = dict(extra_env)
        self.logger = FakeLogger()

    async def exec_as_agent(
        self,
        environment: "FakeEnvironment",
        *,
        command: str,
        timeout_sec: int | None = None,
    ) -> FakeProcess:
        result = await environment.exec(command=command, timeout_sec=timeout_sec)
        if result.return_code != 0:
            raise FakeNonZeroExit(result.return_code)
        return result

    async def exec_as_root(
        self,
        environment: "FakeEnvironment",
        *,
        command: str,
        timeout_sec: int | None = None,
    ) -> FakeProcess:
        result = await environment.exec(command=command, timeout_sec=timeout_sec)
        if result.return_code != 0:
            raise FakeNonZeroExit(result.return_code)
        return result


def _identity_decorator(function):
    return function


def _install_harbor_stubs() -> None:
    modules = {
        name: types.ModuleType(name)
        for name in (
            "harbor",
            "harbor.agents",
            "harbor.agents.installed",
            "harbor.agents.installed.base",
            "harbor.environments",
            "harbor.environments.base",
            "harbor.models",
            "harbor.models.agent",
            "harbor.models.agent.context",
            "harbor.models.task",
            "harbor.models.task.config",
            "harbor.models.trial",
            "harbor.models.trial.config",
            "harbor.models.trial.paths",
        )
    }
    for name, module in modules.items():
        if name.rpartition(".")[2] not in {"base", "context", "paths"}:
            module.__path__ = []
        sys.modules[name] = module
    modules["harbor.agents.installed.base"].BaseInstalledAgent = (
        FakeBaseInstalledAgent
    )
    modules["harbor.agents.installed.base"].NonZeroAgentExitCodeError = (
        FakeNonZeroExit
    )
    modules["harbor.agents.installed.base"].NetworkConnectionError = (
        FakeNetworkConnectionError
    )
    modules["harbor.agents.installed.base"].UnknownApiError = FakeUnknownApiError
    modules["harbor.agents.installed.base"].with_prompt_template = (
        _identity_decorator
    )
    modules["harbor.environments.base"].BaseEnvironment = object
    modules["harbor.models.agent.context"].AgentContext = FakeAgentContext
    modules["harbor.models.task.config"].TaskConfig = FakeTaskConfig
    modules["harbor.models.trial.config"].TrialConfig = FakeTrialConfig
    modules["harbor.models.trial.paths"].EnvironmentPaths = SimpleNamespace(
        agent_dir=PurePosixPath("/logs/agent")
    )


_install_harbor_stubs()
from integrations.harbor import agent  # noqa: E402


class FakeEnvironment:
    def __init__(
        self,
        *,
        runner_return_code: int = 0,
        cleanup_return_code: int = 0,
    ) -> None:
        self.runner_return_code = runner_return_code
        self.cleanup_return_code = cleanup_return_code
        self.commands: list[tuple[str, int | None]] = []
        self.uploads: list[tuple[str, bytes, int]] = []
        self.remote_files: dict[str, bytes] = {}
        self.runner_error: BaseException | None = None
        self.cleanup_error: BaseException | None = None
        self.cancel_cleanup_error: BaseException | None = None
        self._metadata = Path(tempfile.mkdtemp(prefix="s-code-harbor-fake-metadata."))
        metadata = self._metadata
        self.environment_dir = metadata / "task" / "environment"
        self.environment_dir.mkdir(parents=True)
        (self.environment_dir.parent / "task.toml").write_text(
            "[agent]\ntimeout_sec = 3600.0\n",
            encoding="utf-8",
        )
        trial_dir = metadata / "trial"
        trial_dir.mkdir()
        config_path = trial_dir / "config.json"
        config_path.write_text(
            json.dumps(
                {
                    "timeout_multiplier": 1.0,
                    "agent_timeout_multiplier": None,
                    "agent": {
                        "override_timeout_sec": None,
                        "max_timeout_sec": None,
                    },
                }
            ),
            encoding="utf-8",
        )
        self.trial_paths = SimpleNamespace(config_path=config_path)

    def __del__(self) -> None:
        shutil.rmtree(self._metadata, ignore_errors=True)

    def configure_deadline(
        self,
        *,
        task_timeout: float = 3_600.0,
        override_timeout: float | None = None,
        max_timeout: float | None = None,
        timeout_multiplier: float = 1.0,
        agent_timeout_multiplier: float | None = None,
    ) -> None:
        (self.environment_dir.parent / "task.toml").write_text(
            f"[agent]\ntimeout_sec = {task_timeout}\n",
            encoding="utf-8",
        )
        self.trial_paths.config_path.write_text(
            json.dumps(
                {
                    "timeout_multiplier": timeout_multiplier,
                    "agent_timeout_multiplier": agent_timeout_multiplier,
                    "agent": {
                        "override_timeout_sec": override_timeout,
                        "max_timeout_sec": max_timeout,
                    },
                }
            ),
            encoding="utf-8",
        )

    async def exec(
        self,
        *,
        command: str,
        timeout_sec: int | None = None,
    ) -> FakeProcess:
        self.commands.append((command, timeout_sec))
        if command == "command -v id >/dev/null && id -u && id -g":
            return FakeProcess(stdout="1234\n5678\n")
        if command == agent._RUNNER_PATH:
            if self.runner_error is not None:
                raise self.runner_error
            return FakeProcess(return_code=self.runner_return_code)
        if command.startswith("status=0; "):
            if self.cancel_cleanup_error is not None:
                raise self.cancel_cleanup_error
            return FakeProcess()
        if command == f"{agent._INSTALL_DIR}/s-code stop":
            if self.cleanup_error is not None:
                raise self.cleanup_error
            return FakeProcess(return_code=self.cleanup_return_code)
        return FakeProcess()

    async def upload_file(self, source: Path, target: str) -> None:
        source_path = Path(source)
        contents = source_path.read_bytes()
        mode = source_path.stat().st_mode & 0o777
        self.uploads.append((target, contents, mode))
        self.remote_files[target] = contents

    async def download_file(self, source: str, target: Path) -> None:
        if source not in self.remote_files:
            raise FileNotFoundError(source)
        Path(target).write_bytes(self.remote_files[source])


def _manifest(archive_sha256: str) -> SimpleNamespace:
    files = {
        name: SimpleNamespace(sha256=(str(index) * 64), size=index, mode=0o755)
        for index, name in enumerate(agent._EXECUTABLE_MEMBERS, start=1)
    }
    return SimpleNamespace(
        schema_version=1,
        bundle_format="s-code-harbor-bundle",
        source_commit="a" * 40,
        source_tree="b" * 40,
        source_dirty=False,
        target="x86_64-unknown-linux-gnu",
        glibc_version="glibc 2.36",
        lock_sha256s={
            "Cargo.lock": "c" * 64,
            "web/package-lock.json": "d" * 64,
        },
        tool_versions={
            "cargo": "cargo 1.89.0",
            "node": "v22.23.2",
            "npm": "10.9.2",
            "python": "3.12.0",
            "rustc": "rustc 1.89.0",
            "zlib": "1.3.1",
        },
        cargo_features=(agent.HARBOR_CONTAINER_ENV_FEATURE,),
        files=files,
        archive_sha256=archive_sha256,
    )


_COMMAND_ENV_SUMMARY = {
    "count": 2,
    "mode": "container",
    "names": ["HOME", "PATH"],
    "sha256": "f" * 64,
}


class AdapterTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory(prefix="s-code-harbor-adapter.")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.bundle = self.directory / "runtime.tar.gz"
        self.bundle.write_bytes(b"authenticated archive fixture\n")
        self.bundle_sha256 = "e" * 64
        self.manifest = _manifest(self.bundle_sha256)

        version_patcher = mock.patch.object(
            agent, "distribution_version", return_value="0.17.0"
        )
        validate_patcher = mock.patch.object(
            agent, "validate_bundle", return_value=self.manifest
        )
        self.version = version_patcher.start()
        self.validate = validate_patcher.start()
        self.addCleanup(version_patcher.stop)
        self.addCleanup(validate_patcher.stop)

    def new_agent(self, **overrides: object) -> agent.SCodeAgent:
        arguments: dict[str, object] = {
            "logs_dir": self.directory / "logs",
            "model_name": "fixture-model",
            "bundle_path": self.bundle,
            "bundle_sha256": self.bundle_sha256,
            "model_base_url": "https://models.example/v1",
            "credential_env": "S_CODE_HARBOR_API_KEY",
            "extra_env": {"S_CODE_HARBOR_API_KEY": "top-secret-fixture"},
        }
        arguments.update(overrides)
        return agent.SCodeAgent(**arguments)

    def write_events(self, *events: object) -> Path:
        path = self.directory / "events.jsonl"
        path.write_text(
            "".join(json.dumps(event) + "\n" for event in events),
            encoding="utf-8",
        )
        return path

    def test_exact_harbor_version_is_required(self) -> None:
        self.new_agent()
        self.version.assert_called_with("harbor")

        self.version.return_value = "0.17.1"
        with self.assertRaisesRegex(RuntimeError, r"harbor==0\.17\.0; found 0\.17\.1"):
            self.new_agent()

    def test_integer_configuration_bounds_and_types(self) -> None:
        invalid_values = (
            ("max_model_calls", 0),
            ("max_model_calls", 10_001),
            ("max_tool_calls", 0),
            ("max_tool_calls", 10_001),
            ("model_stream_idle_seconds", 0),
            ("model_stream_idle_seconds", 3_601),
            ("model_connect_timeout_sec", 0),
            ("model_connect_timeout_sec", 3_601),
            ("model_read_timeout_sec", 0),
            ("model_read_timeout_sec", 3_601),
            ("model_request_timeout_sec", 0),
            ("model_request_timeout_sec", 3_601),
            ("max_cost_micros", 0),
            ("max_cost_micros", 1_000_000_000_001),
            ("max_total_tokens", 0),
            ("max_total_tokens", 1_000_000_001),
        )
        for name, value in invalid_values:
            with self.subTest(name=name, value=value):
                with self.assertRaises(ValueError):
                    self.new_agent(**{name: value})

        for name in (
            "max_model_calls",
            "max_tool_calls",
            "model_stream_idle_seconds",
            "model_connect_timeout_sec",
            "model_read_timeout_sec",
            "model_request_timeout_sec",
            "max_cost_micros",
            "max_total_tokens",
        ):
            with self.subTest(name=name, value=True):
                with self.assertRaises(TypeError):
                    self.new_agent(**{name: True})

        bounded = self.new_agent(
            max_model_calls=10_000,
            max_tool_calls=10_000,
            model_stream_idle_seconds=3_600,
            model_connect_timeout_sec=3_600,
            model_read_timeout_sec=3_600,
            model_request_timeout_sec=3_600,
            max_cost_micros=1_000_000_000_000,
            max_total_tokens=1_000_000_000,
        )
        self.assertEqual(bounded._model_connect_timeout_sec, 3_450)
        self.assertEqual(bounded._core_timeout_sec, 3_480)
        self.assertEqual(bounded._cli_timeout_sec, 3_510)

    def test_url_and_credential_validation(self) -> None:
        for url in (
            "https://models.example/v1",
            "http://localhost:8000/v1",
            "http://127.0.0.1:8000/v1",
            "http://[::1]:8000/v1",
        ):
            with self.subTest(valid=url):
                self.assertEqual(agent._model_base_url(url), url)

        for url in (
            "http://models.example/v1",
            "https://user:pass@models.example/v1",
            "https://models.example/v1?secret=yes",
            "https://models.example/v1#fragment",
            "ftp://models.example/v1",
            " https://models.example/v1",
            "https://models.example:bad/v1",
        ):
            with self.subTest(invalid=url):
                with self.assertRaises(ValueError):
                    agent._model_base_url(url)

        for name in (
            "S_CODE_HARBOR_API_KEY",
            "S_CODE_HARBOR_MODEL_SECRET",
            "S_CODE_HARBOR_ACCESS_TOKEN",
            "S_CODE_HARBOR_MODEL_PASSWORD",
            "S_CODE_HARBOR_CREDENTIAL",
            "S_CODE_HARBOR_MODEL_AUTH",
        ):
            with self.subTest(valid=name):
                self.assertEqual(agent._credential_env(name), name)

        for name in (
            "S_CODE_HARBOR_MODEL_API",
            "s_code_harbor_model_secret",
            "S_CODE_HARBOR_MODEL_secret",
            "S_CODE_HARBOR_API-KEY",
            "",
            None,
        ):
            with self.subTest(invalid=name):
                with self.assertRaises(ValueError):
                    agent._credential_env(name)

        # Commands never receive S_CODE_* names, so the credential needs one,
        # and S_CODE_HARBOR_ keeps it clear of S-Code's own settings.
        for name in (
            "API_KEY",
            "HARBOR_MODEL_TOKEN",
            "S_CODEX_API_KEY",
            "S_CODE_API_KEY",
            "S_CODE_TOKEN",
            "S_CODE_STORAGE_ENCRYPTION_KEY_BASE64",
        ):
            with self.subTest(unprefixed=name):
                with self.assertRaisesRegex(ValueError, "must start with S_CODE_HARBOR_"):
                    agent._credential_env(name)

        with self.assertRaisesRegex(ValueError, "non-empty value"):
            self.new_agent(extra_env={"S_CODE_HARBOR_API_KEY": ""})

    def test_constructor_manages_environment_and_timeout_hierarchy(self) -> None:
        instance = self.new_agent(extra_fixture="passed-through")
        initialized = instance.base_init
        self.assertEqual(initialized["model_name"], "fixture-model")
        self.assertEqual(
            initialized["version"], f"0.1.0+{agent._ADAPTER_SOURCE_SHA256[:12]}"
        )
        self.assertEqual(initialized["kwargs"], {"extra_fixture": "passed-through"})
        environment = initialized["extra_env"]
        self.assertEqual(environment["S_CODE_HARBOR_API_KEY"], "top-secret-fixture")
        self.assertEqual(environment["S_CODE_TURN_MAX_MODEL_CALLS"], "100")
        self.assertEqual(environment["S_CODE_TURN_MAX_TOOL_CALLS"], "256")
        self.assertEqual(environment["S_CODE_TURN_MAX_ELAPSED_SECONDS"], "3480")
        self.assertEqual(environment["S_CODE_HARBOR_CLI_TIMEOUT_SEC"], "3510")
        self.assertEqual(environment["S_CODE_TURN_MAX_TOTAL_TOKENS"], "10000000")
        self.assertEqual(environment["S_CODE_MODEL_CONNECT_TIMEOUT_SECONDS"], "10")
        self.assertEqual(environment["S_CODE_MODEL_READ_TIMEOUT_SECONDS"], "600")
        self.assertEqual(environment["S_CODE_MODEL_REQUEST_TIMEOUT_SECONDS"], "3450")
        self.assertEqual(environment["S_CODE_MODEL_MERGE_SYSTEM_MESSAGES"], "true")
        self.assertEqual(
            environment["S_CODE_TURN_DISABLED_TOOLS"],
            "web_open,pdf_read,pdf_view,request_user_input",
        )
        self.assertEqual(instance._harbor_timeout_sec, 3600)
        self.assertEqual(instance._core_timeout_sec, 3480)
        self.assertEqual(instance._cli_timeout_sec, 3510)

        with self.assertRaisesRegex(ValueError, "managed by SCodeAgent"):
            self.new_agent(
                extra_env={
                    "S_CODE_HARBOR_API_KEY": "top-secret-fixture",
                    "S_CODE_TURN_MAX_MODEL_CALLS": "999",
                }
            )

    def test_command_environment_settings_are_managed(self) -> None:
        instance = self.new_agent(
            credential_env="S_CODE_HARBOR_CANARY_API_KEY",
            extra_env={
                "S_CODE_HARBOR_CANARY_API_KEY": "canary-credential-value",
                "HTTPS_PROXY": "http://proxy.example:3128",
            },
        )
        environment = instance.base_init["extra_env"]
        self.assertEqual(
            environment["S_CODE_HARBOR_COMMAND_ENV_FILE"],
            "/installed-agent/s-code-private/command.env",
        )
        self.assertEqual(
            environment["S_CODE_HARBOR_COMMAND_ENV_SUMMARY"],
            "/installed-agent/s-code-private/run/command-env.json",
        )
        self.assertNotIn("TERM", environment)
        self.assertNotIn("NO_COLOR", environment)
        # The daemon drops every name Harbor passes to the agent, including the
        # credential and agent env that is not S-Code's own.
        names = environment["S_CODE_HARBOR_AGENT_ENV_NAMES"]
        self.assertEqual(names.split(","), sorted(environment))
        self.assertIn("S_CODE_HARBOR_CANARY_API_KEY", names.split(","))
        self.assertIn("HTTPS_PROXY", names.split(","))
        self.assertNotIn("canary-credential-value", names)

        for name in (
            "S_CODE_HARBOR_COMMAND_ENV_FILE",
            "S_CODE_HARBOR_COMMAND_ENV_SUMMARY",
            "S_CODE_HARBOR_AGENT_ENV_NAMES",
        ):
            with self.subTest(name=name):
                with self.assertRaisesRegex(ValueError, "managed by SCodeAgent"):
                    self.new_agent(
                        extra_env={
                            "S_CODE_HARBOR_API_KEY": "top-secret-fixture",
                            name: "/tmp/override",
                        }
                    )
        with self.assertRaisesRegex(ValueError, "must not contain commas"):
            self.new_agent(
                extra_env={"S_CODE_HARBOR_API_KEY": "top-secret-fixture", "A,B": "x"}
            )

    def test_bundle_without_the_container_env_feature_is_refused(self) -> None:
        for features in ((), ("s-code-cli/fixture",)):
            with self.subTest(features=features):
                self.manifest.cargo_features = features
                with self.assertRaisesRegex(ValueError, "harbor-container-env"):
                    self.new_agent()

    def test_adapter_version_identifies_the_adapter_source(self) -> None:
        sources = self.directory / "adapter-source"
        sources.mkdir()
        (sources / "agent.py").write_bytes(b"agent fixture\n")
        (sources / "__init__.py").write_bytes(b"package fixture\n")
        (sources / ".#agent.py").write_bytes(b"editor lock, not a source\n")
        (sources / "README.md").write_bytes(b"not a source\n")
        listing = "".join(
            f"{hashlib.sha256(contents).hexdigest()}  {name}\n"
            for name, contents in (
                ("__init__.py", b"package fixture\n"),
                ("agent.py", b"agent fixture\n"),
            )
        )
        expected = hashlib.sha256(listing.encode()).hexdigest()
        self.assertEqual(agent._adapter_source_sha256(sources), expected)
        (sources / "agent.py").write_bytes(b"changed agent\n")
        self.assertNotEqual(agent._adapter_source_sha256(sources), expected)

        adapter_directory = ROOT / "integrations" / "harbor"
        self.assertEqual(
            agent._ADAPTER_SOURCE_SHA256, agent._adapter_source_sha256(adapter_directory)
        )
        # The README's shell recipe reproduces the digest.
        for tool in ("sha256sum", "shasum -a 256"):
            if shutil.which(tool.split()[0]) is None:
                continue
            recipe = subprocess.run(
                ["/bin/sh", "-c", f"{tool} __init__.py agent.py runtime.py | {tool}"],
                cwd=adapter_directory,
                env={**os.environ, "LC_ALL": "C"},
                check=True,
                capture_output=True,
                text=True,
            )
            self.assertEqual(recipe.stdout.split()[0], agent._ADAPTER_SOURCE_SHA256)
            break

    def test_harbor_deadline_resolution_matches_cap_then_multiplier(self) -> None:
        environment = FakeEnvironment()
        environment.configure_deadline(
            task_timeout=900,
            override_timeout=800,
            max_timeout=700,
            timeout_multiplier=3,
            agent_timeout_multiplier=2,
        )
        self.assertEqual(
            agent._resolved_harbor_agent_timeout_sec(environment),
            1_400,
        )

        environment.configure_deadline(
            task_timeout=900,
            override_timeout=800,
            max_timeout=700,
            timeout_multiplier=3,
        )
        self.assertEqual(
            agent._resolved_harbor_agent_timeout_sec(environment),
            2_100,
        )

        environment.configure_deadline(task_timeout=179)
        with self.assertRaisesRegex(ValueError, "between 180 and 86400"):
            agent._resolved_harbor_agent_timeout_sec(environment)

        environment.trial_paths.config_path.write_text("not json", encoding="utf-8")
        with self.assertRaisesRegex(RuntimeError, "could not resolve"):
            agent._resolved_harbor_agent_timeout_sec(environment)

    def test_sampling_kwargs_are_optional_and_validated(self) -> None:
        environment = self.new_agent().base_init["extra_env"]
        self.assertNotIn("S_CODE_MODEL_TEMPERATURE", environment)
        self.assertNotIn("S_CODE_MODEL_MAX_OUTPUT_TOKENS", environment)

        configured = self.new_agent(temperature=0.6, max_output_tokens=4096)
        environment = configured.base_init["extra_env"]
        self.assertEqual(environment["S_CODE_MODEL_TEMPERATURE"], "0.6")
        self.assertEqual(environment["S_CODE_MODEL_MAX_OUTPUT_TOKENS"], "4096")
        execution = configured._provenance()["execution"]
        self.assertEqual(execution["temperature"], 0.6)
        self.assertEqual(execution["max_output_tokens"], 4096)

        for kwargs, error in (
            ({"temperature": 2.5}, ValueError),
            ({"temperature": float("nan")}, ValueError),
            ({"temperature": True}, TypeError),
            ({"temperature": "0.6"}, TypeError),
            ({"max_output_tokens": 0}, ValueError),
            ({"max_output_tokens": True}, TypeError),
        ):
            with self.subTest(kwargs=kwargs):
                with self.assertRaises(error):
                    self.new_agent(**kwargs)

    def test_install_uses_one_archive_and_numeric_nonroot_owner(self) -> None:
        instance = self.new_agent()
        environment = FakeEnvironment()
        environment.configure_deadline(task_timeout=600)
        asyncio.run(instance.install(environment))

        self.assertEqual(instance._agent_owner, "1234:5678")
        self.assertEqual(instance._harbor_timeout_sec, 600)
        self.assertEqual(instance._model_connect_timeout_sec, 10)
        self.assertEqual(instance._model_read_timeout_sec, 450)
        self.assertEqual(instance._model_request_timeout_sec, 450)
        self.assertEqual(instance._model_stream_idle_seconds, 300)
        self.assertEqual(instance._core_timeout_sec, 480)
        self.assertEqual(instance._cli_timeout_sec, 510)
        self.assertEqual(
            instance._extra_env["S_CODE_TURN_MAX_ELAPSED_SECONDS"], "480"
        )
        self.assertEqual(
            instance._extra_env["S_CODE_MODEL_REQUEST_TIMEOUT_SECONDS"], "450"
        )
        self.assertEqual(
            instance._provenance()["execution"]["output_mode"],
            "raw_events_jsonl",
        )
        self.assertEqual(self.validate.call_count, 2)
        targets = [target for target, _, _ in environment.uploads]
        self.assertEqual(targets, [agent._BUNDLE_ARCHIVE_PATH, agent._RUNNER_PATH])
        archive_upload = environment.uploads[0]
        self.assertEqual(archive_upload[1], self.bundle.read_bytes())
        runner_upload = environment.uploads[1]
        self.assertEqual(runner_upload[1], agent._RUNNER.encode("utf-8"))
        self.assertEqual(runner_upload[2], 0o755)

        commands = "\n".join(command for command, _ in environment.commands)
        self.assertIn("getconf GNU_LIBC_VERSION", commands)
        self.assertIn("env -0 >/dev/null", commands)
        self.assertIn(self.bundle_sha256, commands)
        self.assertIn("sha256sum -c -", commands)
        self.assertIn(
            f"tar -xzf {agent._BUNDLE_ARCHIVE_PATH} -C {agent._INSTALL_DIR}",
            commands,
        )
        self.assertIn(f"chown -- 1234:5678 {agent._PRIVATE_DIR}", commands)
        self.assertIn("s-code --help", commands)
        for executable in ("s-code-cli", "s-code-daemon", "s-code-pdf-worker"):
            self.assertIn(f"{executable} --self-test", commands)

        secret = b"top-secret-fixture"
        self.assertNotIn(secret.decode(), commands)
        for _, contents, _ in environment.uploads:
            self.assertNotIn(secret, contents)

    def test_install_keeps_the_agent_env_name_list_complete(self) -> None:
        instance = self.new_agent()
        timeout_env = instance._effective_timeout_env
        instance._effective_timeout_env = lambda: {
            **timeout_env(),
            "S_CODE_FIXTURE_ADDED_AT_INSTALL": "1",
        }
        asyncio.run(instance.install(FakeEnvironment()))
        names = instance._extra_env["S_CODE_HARBOR_AGENT_ENV_NAMES"].split(",")
        self.assertIn("S_CODE_FIXTURE_ADDED_AT_INSTALL", names)
        self.assertEqual(names, sorted(instance._extra_env))

    def _run_generated_runner(
        self,
        *,
        exec_status: int,
        cleanup_status: int,
        prompt: bytes,
        command_env_file: str | None = "command.env",
    ) -> tuple[subprocess.CompletedProcess[bytes], Path, Path, Path]:
        case_directory = Path(
            tempfile.mkdtemp(prefix="runner-case.", dir=self.directory)
        )
        logs = case_directory / "logs"
        private = case_directory / "private"
        logs.mkdir()
        private.mkdir()
        fake_s_code = case_directory / "fake-s-code"
        calls = case_directory / "calls"
        captured_prompt = case_directory / "captured-prompt"
        captured_stdin = case_directory / "captured-stdin"
        fake_s_code.write_text(
            """#!/bin/sh
command=$1
shift
case "$command" in
  exec)
    printf 'exec\\n' >> "$FAKE_CALLS"
    cp "$S_CODE_HARBOR_COMMAND_ENV_FILE" "$FAKE_CAPTURED_ENV" || exit 97
    ls -ln "$S_CODE_HARBOR_COMMAND_ENV_FILE" > "$FAKE_CAPTURED_ENV_MODE"
    printf '%s\\n' "$FAKE_SUMMARY" > "$S_CODE_HARBOR_COMMAND_ENV_SUMMARY"
    cat > "$FAKE_CAPTURED_STDIN"
    [ ! -s "$FAKE_CAPTURED_STDIN" ] || exit 98
    last=
    for argument do last=$argument; done
    printf '%s' "$last" > "$FAKE_CAPTURED_PROMPT"
    printf '%s\\n' '{"kind":"turn.completed","payload":{}}'
    printf '%s\\n' 'fixture stderr' >&2
    mkdir -p "$FAKE_STATE_DIR/logs"
    printf '%s\\n' 'fixture daemon log' > "$FAKE_STATE_DIR/logs/daemon.log"
    exit "$FAKE_EXEC_STATUS"
    ;;
  stop)
    printf 'stop\\n' >> "$FAKE_CALLS"
    exit "$FAKE_CLEANUP_STATUS"
    ;;
  *) exit 99 ;;
esac
""",
            encoding="utf-8",
        )
        fake_s_code.chmod(0o755)
        replacements = {
            f"{agent._INSTALL_DIR}/s-code": str(fake_s_code),
            agent._EVENTS_PATH: str(logs / "events.jsonl"),
            agent._STDERR_PATH: str(logs / "stderr.log"),
            agent._FINAL_PATH: str(logs / "final.txt"),
            agent._PROVENANCE_PATH: str(logs / "provenance.json"),
            agent._DAEMON_LOG_PATH: str(logs / "daemon.log"),
            agent._COMMAND_ENV_PATH: str(logs / "command-env.json"),
            agent._PRIVATE_DIR: str(private),
        }
        rendered = agent._RUNNER
        for original in sorted(replacements, key=len, reverse=True):
            rendered = rendered.replace(original, replacements[original])
        runner = case_directory / "runner"
        runner.write_text(rendered, encoding="utf-8")
        runner.chmod(0o755)
        (private / "instruction.txt").write_bytes(prompt)
        (private / "provenance.seed.json").write_text(
            '{"daemon_cleanup_exit_code": null, "exit_code": null, '
            '"runner_exit_code": null}\n',
            encoding="utf-8",
        )

        child_env = dict(os.environ)
        child_env.update(
            {
                "FAKE_CALLS": str(calls),
                "FAKE_CAPTURED_PROMPT": str(captured_prompt),
                "FAKE_CAPTURED_STDIN": str(captured_stdin),
                "FAKE_EXEC_STATUS": str(exec_status),
                "FAKE_CLEANUP_STATUS": str(cleanup_status),
                "FAKE_STATE_DIR": str(private / "state"),
                "FAKE_CAPTURED_ENV": str(case_directory / "captured-env"),
                "FAKE_CAPTURED_ENV_MODE": str(case_directory / "captured-env-mode"),
                "FAKE_SUMMARY": json.dumps(_COMMAND_ENV_SUMMARY),
                "HARBOR_TASK_MARKER": "task value\nwith a newline",
                # An image variable named like one of the runner's own.
                "events": "image value",
                # Variables Harbor passes only to the agent.
                "S_CODE_HARBOR_CANARY_API_KEY": "canary-credential-value",
                "AGENT_ONLY_MARKER": "agent-only-value",
                "S_CODE_HARBOR_AGENT_ENV_NAMES": (
                    "AGENT_ONLY_MARKER,S_CODE_HARBOR_AGENT_ENV_NAMES,"
                    "S_CODE_HARBOR_CANARY_API_KEY"
                ),
                "S_CODE_HARBOR_CLI_TIMEOUT_SEC": "3570",
                "S_CODE_HARBOR_COMMAND_ENV_SUMMARY": str(
                    private / "run" / "command-env.json"
                ),
            }
        )
        if command_env_file is not None:
            child_env["S_CODE_HARBOR_COMMAND_ENV_FILE"] = str(private / command_env_file)
        result = subprocess.run(
            ["/bin/sh", str(runner)],
            env=child_env,
            input=b"parent stdin must not reach noninteractive S-Code",
            check=False,
            capture_output=True,
        )
        return result, private / "exit.status", calls, captured_prompt

    def test_generated_runner_syntax_and_prompt_data_handling(self) -> None:
        self.assertIn(" --raw-events-jsonl ", agent._RUNNER)
        self.assertNotIn(" --stream-json ", agent._RUNNER)
        syntax = subprocess.run(
            ["/bin/sh", "-n"],
            input=agent._RUNNER.encode("utf-8"),
            check=False,
            capture_output=True,
        )
        self.assertEqual(syntax.returncode, 0, syntax.stderr.decode())

        injection_marker = self.directory / "prompt-was-executed"
        prompt = f"$(touch {injection_marker}) ; `false`\n\n".encode()
        result, exit_status, calls, captured_prompt = self._run_generated_runner(
            exec_status=0,
            cleanup_status=7,
            prompt=prompt,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertEqual(exit_status.read_text(encoding="ascii"), "0 7 0\n")
        self.assertEqual(calls.read_text(encoding="ascii"), "exec\nstop\n")
        self.assertEqual(captured_prompt.read_bytes(), prompt)
        self.assertFalse(injection_marker.exists())
        daemon_log = exit_status.parent.parent / "logs" / "daemon.log"
        self.assertEqual(daemon_log.read_text(encoding="utf-8"), "fixture daemon log\n")
        self.assertEqual(daemon_log.stat().st_mode & 0o777, 0o600)

        result, exit_status, calls, captured_prompt = self._run_generated_runner(
            exec_status=42,
            cleanup_status=7,
            prompt=b"second fixture\n",
        )
        self.assertEqual(result.returncode, 42)
        self.assertEqual(exit_status.read_text(encoding="ascii"), "42 7 42\n")
        self.assertEqual(calls.read_text(encoding="ascii"), "exec\nstop\n")
        self.assertEqual(captured_prompt.read_bytes(), b"second fixture\n")

    def test_generated_runner_captures_the_command_environment_first(self) -> None:
        # The snapshot is taken before the runner assigns any variable of its
        # own, and checked before S-Code starts.
        self.assertLess(
            agent._RUNNER.index('exec env "$@" -0'), agent._RUNNER.index("\nevents=")
        )
        self.assertLess(
            agent._RUNNER.index('[ "$s_code_runner_captured" -eq 0 ]'),
            agent._RUNNER.index(f"{agent._INSTALL_DIR}/s-code exec"),
        )
        result, exit_status, calls, _ = self._run_generated_runner(
            exec_status=0,
            cleanup_status=0,
            prompt=b"task\n",
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        case = exit_status.parent.parent
        # S-Code saw a private, NUL-separated snapshot when it started.
        records = (case / "captured-env").read_bytes().split(b"\0")
        self.assertIn(b"HARBOR_TASK_MARKER=task value\nwith a newline", records)
        # The image's value, not the runner's, and no agent-only variable.
        self.assertIn(b"events=image value", records)
        names = {record.split(b"=", 1)[0] for record in records if record}
        for agent_only in (
            b"AGENT_ONLY_MARKER",
            b"S_CODE_HARBOR_CANARY_API_KEY",
            b"S_CODE_HARBOR_AGENT_ENV_NAMES",
        ):
            self.assertNotIn(agent_only, names)
        self.assertNotIn(b"canary-credential-value", b"\0".join(records))
        mode = (case / "captured-env-mode").read_text(encoding="utf-8")
        self.assertTrue(mode.startswith("-rw-------"), mode)
        # The runner removes a snapshot the daemon did not delete.
        self.assertFalse((exit_status.parent / "command.env").exists())
        summary = case / "logs" / "command-env.json"
        self.assertEqual(
            json.loads(summary.read_text(encoding="utf-8")), _COMMAND_ENV_SUMMARY
        )
        self.assertEqual(summary.stat().st_mode & 0o777, 0o600)

        # Without a snapshot S-Code must not start, whether the capture fails
        # or the adapter did not ask for one.
        for command_env_file in ("missing/command.env", None):
            with self.subTest(command_env_file=command_env_file):
                self._assert_runner_refuses_without_snapshot(command_env_file)

    def _assert_runner_refuses_without_snapshot(self, command_env_file: str | None) -> None:
        result, exit_status, calls, _ = self._run_generated_runner(
            exec_status=0,
            cleanup_status=0,
            prompt=b"task\n",
            command_env_file=command_env_file,
        )
        self.assertEqual(result.returncode, 1)
        self.assertFalse(calls.exists())
        self.assertEqual(exit_status.read_text(encoding="ascii"), "1 0 1\n")
        stderr_log = exit_status.parent.parent / "logs" / "stderr.log"
        self.assertIn(
            "could not capture the command environment",
            stderr_log.read_text(encoding="utf-8"),
        )

    def test_event_stream_classifications(self) -> None:
        completed = agent._classify_event_stream(
            self.write_events(
                {
                    "kind": "turn.usage",
                    "payload": {
                        "input_units": 12,
                        "output_units": 7,
                        "model_calls": 2,
                        "tool_calls": 3,
                    },
                },
                {"kind": "turn.completed", "sequence": 9, "payload": {}},
            ),
            0,
        )
        self.assertEqual(completed["status"], "completed")
        self.assertEqual(
            completed["usage"],
            {"input_tokens": 12, "output_tokens": 7, "model_calls": 2, "tool_calls": 3},
        )
        self.assertEqual(completed["event_stream"]["validation"], "valid")

        cases = (
            (
                {"kind": "question.required", "payload": {"source": "tool_call_limit"}},
                1,
                "limit_stopped",
                "tool_call_limit",
                None,
            ),
            (
                {"kind": "turn.failed", "payload": {"reason": "model-call limit reached"}},
                1,
                "limit_stopped",
                "model_call_limit",
                None,
            ),
            (
                {"kind": "turn.failed", "error_code": "model_stream_idle_timeout"},
                1,
                "timed_out",
                "model_stream_idle_timeout",
                None,
            ),
            (
                {"kind": "turn.failed", "payload": {"error_code": "turn_elapsed_timeout"}},
                1,
                "timed_out",
                "turn_elapsed_timeout",
                None,
            ),
            (
                {"kind": "turn.cancelled", "payload": {}},
                130,
                "cancelled",
                None,
                None,
            ),
            (
                {"kind": "question.required", "payload": {"source": "model"}},
                1,
                "failed",
                None,
                "input_required",
            ),
            (
                {"kind": "approval.required", "payload": {}},
                1,
                "failed",
                None,
                "approval_required",
            ),
        )
        for event, exit_code, status, limit_kind, failure_kind in cases:
            with self.subTest(kind=event["kind"], status=status, limit_kind=limit_kind):
                result = agent._classify_event_stream(
                    self.write_events(event), exit_code
                )
                self.assertEqual(result["status"], status)
                self.assertEqual(result["limit_kind"], limit_kind)
                self.assertEqual(result["failure_kind"], failure_kind)
                self.assertEqual(result["event_stream"]["validation"], "valid")

        malformed = self.directory / "malformed.jsonl"
        malformed.write_bytes(b"not json\n")
        result = agent._classify_event_stream(malformed, 0)
        self.assertEqual(result["status"], "failed")
        self.assertEqual(result["event_stream"]["validation"], "invalid")
        self.assertEqual(result["event_stream"]["records"], 0)
        self.assertNotIn("not json", result["event_stream"]["error"])

    def test_runs_without_a_model_response_name_the_endpoint(self) -> None:
        def finished(status: str) -> dict[str, object]:
            return {
                "kind": "privacy.request.finished",
                "payload": {"request": {"status": status}},
            }

        failed = {"kind": "turn.failed", "payload": {"error_code": "agent_error"}}
        cases = (
            (("connection_error",) * 3, "model_unreachable"),
            (("rejected",), "model_rejected"),
            (("connection_error", "rejected"), "model_unreachable"),
            (("connection_error", "accepted"), None),
        )
        for outcomes, failure_kind in cases:
            with self.subTest(outcomes=outcomes):
                result = agent._classify_event_stream(
                    self.write_events(*(finished(status) for status in outcomes), failed),
                    1,
                )
                self.assertEqual(result["status"], "failed")
                self.assertEqual(result["failure_kind"], failure_kind)
                self.assertEqual(
                    sum(result["model_requests"].values()), len(outcomes)
                )

        completed = agent._classify_event_stream(
            self.write_events(
                finished("connection_error"),
                {"kind": "turn.completed", "payload": {}},
            ),
            0,
        )
        self.assertEqual(completed["status"], "completed")
        self.assertIsNone(completed["failure_kind"])

    def _run_environment(
        self,
        *,
        runner_return_code: int,
        cleanup_return_code: int,
        status: bytes,
        events: bytes,
    ) -> FakeEnvironment:
        environment = FakeEnvironment(
            runner_return_code=runner_return_code,
            cleanup_return_code=cleanup_return_code,
        )
        environment.remote_files[agent._EXIT_STATUS_PATH] = status
        environment.remote_files[agent._EVENTS_PATH] = events
        return environment

    def test_post_cleanup_failure_precedes_runner_nonzero(self) -> None:
        instance = self.new_agent()
        instance._agent_owner = "1234:5678"
        environment = self._run_environment(
            runner_return_code=42,
            cleanup_return_code=7,
            status=b"42 7 42\n",
            events=b'{"kind":"turn.failed","payload":{"reason":"fixture"}}\n',
        )
        context = FakeAgentContext()
        with self.assertRaisesRegex(RuntimeError, "task state is not quiescent") as raised:
            asyncio.run(instance.run("one task", environment, context))
        self.assertIs(type(raised.exception), FakeNonZeroExit)
        self.assertEqual(
            [command for command, _ in environment.commands][-2:],
            [agent._RUNNER_PATH, f"{agent._INSTALL_DIR}/s-code stop"],
        )
        self.assertIsNone(environment.commands[-2][1])
        self.assertEqual(environment.commands[-1][1], 10)

    def test_harbor_cancellation_is_preserved_after_bounded_cleanup(self) -> None:
        instance = self.new_agent()
        instance._agent_owner = "1234:5678"
        environment = self._run_environment(
            runner_return_code=0,
            cleanup_return_code=0,
            status=b"",
            events=b"",
        )
        cancellation = asyncio.CancelledError("Harbor deadline")
        environment.runner_error = cancellation
        environment.cancel_cleanup_error = RuntimeError("cleanup transport failed")
        with self.assertRaises(asyncio.CancelledError) as raised:
            asyncio.run(instance.run("one task", environment, FakeAgentContext()))
        self.assertIs(raised.exception, cancellation)
        self.assertTrue(environment.commands[-1][0].startswith("status=0; "))
        self.assertIn(f"rm -f {agent._COMMAND_ENV_FILE_PATH};", environment.commands[-1][0])
        self.assertEqual(environment.commands[-1][1], 10)
        self.assertNotIn(agent._PROVENANCE_PATH, environment.remote_files)

    def test_cancellation_during_post_run_cleanup_is_preserved(self) -> None:
        instance = self.new_agent()
        instance._agent_owner = "1234:5678"
        environment = self._run_environment(
            runner_return_code=0,
            cleanup_return_code=0,
            status=b"0 0 0\n",
            events=b'{"kind":"turn.completed","payload":{}}\n',
        )
        cancellation = asyncio.CancelledError("Harbor deadline")
        environment.cleanup_error = cancellation
        with self.assertRaises(asyncio.CancelledError) as raised:
            asyncio.run(instance.run("one task", environment, FakeAgentContext()))
        self.assertIs(raised.exception, cancellation)
        self.assertTrue(environment.commands[-1][0].startswith("status=0; "))

    def test_success_with_invalid_event_evidence_fails_closed(self) -> None:
        instance = self.new_agent()
        instance._agent_owner = "1234:5678"
        environment = self._run_environment(
            runner_return_code=0,
            cleanup_return_code=0,
            status=b"0 0 0\n",
            events=b"malformed event\n",
        )
        context = FakeAgentContext()
        with self.assertRaisesRegex(
            FakeNonZeroExit, "without valid completed-run evidence"
        ):
            asyncio.run(instance.run("one task", environment, context))
        provenance = json.loads(environment.remote_files[agent._PROVENANCE_PATH])
        self.assertEqual(provenance["result"]["event_stream"]["validation"], "invalid")
        self.assertEqual(provenance["execution"]["post_run_cleanup_status"], "succeeded")

    def test_endpoint_failures_use_harbor_retryable_error_types(self) -> None:
        cases = (
            ("connection_error", FakeNetworkConnectionError, "model_unreachable"),
            ("rejected", FakeUnknownApiError, "model_rejected"),
        )
        for request_status, expected_error, failure_kind in cases:
            with self.subTest(request_status=request_status):
                instance = self.new_agent()
                instance._agent_owner = "1234:5678"
                environment = self._run_environment(
                    runner_return_code=1,
                    cleanup_return_code=0,
                    status=b"1 0 1\n",
                    events=(
                        json.dumps(
                            {
                                "kind": "privacy.request.finished",
                                "payload": {"request": {"status": request_status}},
                            }
                        ).encode()
                        + b"\n"
                        + b'{"kind":"turn.failed","payload":{}}\n'
                    ),
                )
                with self.assertRaises(expected_error):
                    asyncio.run(
                        instance.run("one task", environment, FakeAgentContext())
                    )
                provenance = json.loads(
                    environment.remote_files[agent._PROVENANCE_PATH]
                )
                self.assertEqual(provenance["result"]["failure_kind"], failure_kind)
                self.assertEqual(
                    provenance["execution"]["post_run_cleanup_status"], "succeeded"
                )

    def test_untrusted_endpoint_evidence_is_not_promoted(self) -> None:
        instance = self.new_agent()
        instance._agent_owner = "1234:5678"
        environment = self._run_environment(
            runner_return_code=1,
            cleanup_return_code=0,
            status=b"malformed\n",
            events=(
                b'{"kind":"privacy.request.finished","payload":'
                b'{"request":{"status":"connection_error"}}}\n'
                b'{"kind":"turn.failed","payload":{}}\n'
            ),
        )
        with self.assertRaises(FakeNonZeroExit) as raised:
            asyncio.run(instance.run("one task", environment, FakeAgentContext()))
        self.assertIs(type(raised.exception), FakeNonZeroExit)

    def test_finalization_failure_remains_a_harbor_agent_failure(self) -> None:
        instance = self.new_agent()
        instance._agent_owner = "1234:5678"
        environment = self._run_environment(
            runner_return_code=0,
            cleanup_return_code=0,
            status=b"0 0 0\n",
            events=b'{"kind":"turn.completed","payload":{}}\n',
        )
        instance._finalize_provenance = mock.AsyncMock(
            side_effect=OSError("fixture finalization failure")
        )
        with self.assertRaisesRegex(
            FakeNonZeroExit, "could not validate and preserve"
        ):
            asyncio.run(instance.run("one task", environment, FakeAgentContext()))

    def test_successful_run_is_one_shot_and_records_usage(self) -> None:
        instance = self.new_agent()
        instance._agent_owner = "1234:5678"
        environment = self._run_environment(
            runner_return_code=0,
            cleanup_return_code=0,
            status=b"0 0 0\n",
            events=(
                b'{"kind":"turn.usage","payload":{"input_tokens":8,'
                b'"output_tokens":5}}\n'
                b'{"kind":"turn.completed","payload":{}}\n'
            ),
        )
        context = FakeAgentContext()
        asyncio.run(instance.run("one task\n", environment, context))
        self.assertEqual(context.n_input_tokens, 8)
        self.assertEqual(context.n_output_tokens, 5)
        self.assertEqual(context.metadata["s_code"]["result_status"], "completed")
        self.assertEqual(environment.remote_files[agent._PROMPT_PATH], b"one task\n")
        # No summary was preserved, so provenance says why it has none.
        execution = json.loads(environment.remote_files[agent._PROVENANCE_PATH])[
            "execution"
        ]
        self.assertIsNone(execution["command_environment"])
        self.assertEqual(execution["host_command_env"], "unverified")
        self.assertEqual(
            execution["command_environment_error"], "command-env.json is unavailable"
        )

        with self.assertRaisesRegex(RuntimeError, "exactly one run"):
            asyncio.run(instance.run("second task", environment, context))

    def test_provenance_records_the_command_environment_by_name_only(self) -> None:
        instance = self.new_agent(
            credential_env="S_CODE_HARBOR_CANARY_API_KEY",
            extra_env={
                "S_CODE_HARBOR_CANARY_API_KEY": "canary-credential-value",
                "S_CODE_MCP_ENABLED": "canary-mcp-value",
                "HTTPS_PROXY": "http://canary-proxy.example:3128",
            },
        )
        instance._agent_owner = "1234:5678"
        environment = self._run_environment(
            runner_return_code=0,
            cleanup_return_code=0,
            status=b"0 0 0\n",
            events=b'{"kind":"turn.completed","payload":{}}\n',
        )
        environment.remote_files[agent._COMMAND_ENV_PATH] = json.dumps(
            _COMMAND_ENV_SUMMARY
        ).encode()
        context = FakeAgentContext()
        asyncio.run(instance.run("one task", environment, context))
        self.assertEqual(
            context.metadata["s_code"]["command_env"], agent._COMMAND_ENV_PATH
        )

        recorded = environment.remote_files[agent._PROVENANCE_PATH]
        provenance = json.loads(recorded)
        self.assertEqual(
            provenance["adapter"]["version"],
            f"0.1.0+{agent._ADAPTER_SOURCE_SHA256[:12]}",
        )
        self.assertEqual(
            provenance["adapter"]["source_sha256"], agent._ADAPTER_SOURCE_SHA256
        )
        self.assertEqual(
            provenance["bundle"]["cargo_features"],
            [agent.HARBOR_CONTAINER_ENV_FEATURE],
        )
        execution = provenance["execution"]
        self.assertEqual(execution["host_command_env"], "container")
        self.assertEqual(
            execution["command_environment"],
            {"count": 2, "names": ["HOME", "PATH"], "sha256": "f" * 64},
        )
        self.assertIsNone(execution["command_environment_error"])
        self.assertEqual(execution["unmanaged_s_code_env"], ["S_CODE_MCP_ENABLED"])
        self.assertEqual(
            provenance["artifacts"]["command_env"], agent._COMMAND_ENV_PATH
        )
        seed = environment.remote_files[agent._PROVENANCE_SEED_PATH]
        for value in (b"canary-credential-value", b"canary-mcp-value", b"canary-proxy"):
            self.assertNotIn(value, recorded)
            self.assertNotIn(value, seed)

    def test_command_environment_summary_is_validated(self) -> None:
        path = self.directory / "command-env.json"
        self.assertEqual(
            agent._command_environment(path),
            (None, "command-env.json is unavailable"),
        )
        invalid = (
            b"not json",
            b"[]",
            {**_COMMAND_ENV_SUMMARY, "values": {}},
            {**_COMMAND_ENV_SUMMARY, "mode": "minimal"},
            {**_COMMAND_ENV_SUMMARY, "count": 3},
            {**_COMMAND_ENV_SUMMARY, "count": True, "names": ["HOME"]},
            {**_COMMAND_ENV_SUMMARY, "names": ["PATH", "HOME"]},
            # A name carrying "=" could smuggle a value into provenance.
            {**_COMMAND_ENV_SUMMARY, "names": ["HOME=/root", "PATH"]},
            {**_COMMAND_ENV_SUMMARY, "sha256": "F" * 64},
        )
        for contents in invalid:
            with self.subTest(contents=contents):
                if isinstance(contents, dict):
                    contents = json.dumps(contents).encode()
                path.write_bytes(contents)
                summary, error = agent._command_environment(path)
                self.assertIsNone(summary)
                self.assertIsInstance(error, str)

        path.write_text(json.dumps(_COMMAND_ENV_SUMMARY), encoding="utf-8")
        with mock.patch.object(agent, "_MAX_COMMAND_ENV_SUMMARY_BYTES", 8):
            self.assertEqual(
                agent._command_environment(path),
                (None, "command-env.json is too large"),
            )


if __name__ == "__main__":
    unittest.main()
