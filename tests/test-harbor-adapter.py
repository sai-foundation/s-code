#!/usr/bin/env python3
"""Focused standard-library tests for the pinned Harbor adapter."""

from __future__ import annotations

import asyncio
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
        files=files,
        archive_sha256=archive_sha256,
    )


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
            "API_KEY",
            "MODEL_SECRET",
            "ACCESS_TOKEN",
            "MODEL_PASSWORD",
            "MODEL_CREDENTIAL",
            "MODEL_AUTH",
        ):
            with self.subTest(valid=name):
                self.assertEqual(agent._credential_env(name), name)

        for name in (
            "MODEL_API",
            "model_secret",
            "MODEL_secret",
            "9_API_KEY",
            "API-KEY",
            "",
            None,
        ):
            with self.subTest(invalid=name):
                with self.assertRaises(ValueError):
                    agent._credential_env(name)

        with self.assertRaisesRegex(ValueError, "non-empty value"):
            self.new_agent(extra_env={"S_CODE_HARBOR_API_KEY": ""})

    def test_constructor_manages_environment_and_timeout_hierarchy(self) -> None:
        instance = self.new_agent(extra_fixture="passed-through")
        initialized = instance.base_init
        self.assertEqual(initialized["model_name"], "fixture-model")
        self.assertEqual(initialized["version"], "0.1.0")
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

    def _run_generated_runner(
        self,
        *,
        exec_status: int,
        cleanup_status: int,
        prompt: bytes,
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
                "S_CODE_HARBOR_CLI_TIMEOUT_SEC": "3570",
            }
        )
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
        self.assertEqual(environment.commands[-1][1], 10)
        self.assertNotIn(agent._PROVENANCE_PATH, environment.remote_files)

    def test_s_code_time_limit_is_reported_as_a_timeout(self) -> None:
        for exit_status, events in (
            (
                b"1 0 1\n",
                b'{"kind":"turn.failed","payload":{"error_code":"turn_elapsed_timeout"}}\n',
            ),
            (b"124 0 124\n", b'{"kind":"turn.failed","payload":{"error_code":"turn_elapsed_timeout"}}\n'),
        ):
            with self.subTest(exit_status=exit_status):
                instance = self.new_agent()
                instance._agent_owner = "1234:5678"
                code = int(exit_status.split()[0])
                environment = self._run_environment(
                    runner_return_code=code,
                    cleanup_return_code=0,
                    status=exit_status,
                    events=events,
                )
                with self.assertRaises(TimeoutError) as raised:
                    asyncio.run(instance.run("one task", environment, FakeAgentContext()))
                self.assertIsInstance(raised.exception.__cause__, FakeNonZeroExit)

    def test_agent_failure_is_not_reported_as_a_timeout(self) -> None:
        instance = self.new_agent()
        instance._agent_owner = "1234:5678"
        environment = self._run_environment(
            runner_return_code=1,
            cleanup_return_code=0,
            status=b"1 0 1\n",
            events=b'{"kind":"turn.failed","payload":{"error_code":"agent_failed"}}\n',
        )
        with self.assertRaises(FakeNonZeroExit):
            asyncio.run(instance.run("one task", environment, FakeAgentContext()))

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

        with self.assertRaisesRegex(RuntimeError, "exactly one run"):
            asyncio.run(instance.run("second task", environment, context))


if __name__ == "__main__":
    unittest.main()
