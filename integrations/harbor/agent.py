"""Pinned Harbor 0.17 adapter for one isolated S-Code run.

The adapter deliberately has a small surface: a local, checksum-pinned runtime
bundle; one OpenAI-compatible model endpoint; and one credential environment
handle supplied through Harbor's agent environment.  It does not download
software, retry a turn, resume a session, persist state, or claim ATIF support.
"""

from __future__ import annotations

import asyncio
import json
import math
import re
import shlex
import shutil
import tempfile
from importlib.metadata import version as distribution_version
from ipaddress import ip_address
from pathlib import Path
from typing import Any, override
from urllib.parse import urlsplit

from harbor.agents.installed.base import (
    BaseInstalledAgent,
    NetworkConnectionError,
    NonZeroAgentExitCodeError,
    UnknownApiError,
    with_prompt_template,
)
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext
from harbor.models.task.config import TaskConfig as HarborTaskConfig
from harbor.models.trial.config import TrialConfig
from harbor.models.trial.paths import EnvironmentPaths

from .runtime import BundleManifest, validate_bundle, validate_sha256


_SUPPORTED_HARBOR_VERSION = "0.17.0"
_ADAPTER_VERSION = "0.1.0"
_CREDENTIAL_ENV_RE = re.compile(r"^[A-Z_][A-Z0-9_]*$")
_SENSITIVE_ENV_TOKEN_RE = re.compile(
    r"KEY|SECRET|TOKEN|PASSWORD|CREDENTIAL|AUTH",
    re.IGNORECASE,
)
_EXECUTABLE_MEMBERS = (
    "s-code",
    "s-code-cli",
    "s-code-daemon",
    "s-code-pdf-worker",
)
_MAX_CALL_LIMIT = 10_000
_MAX_IDLE_SEC = 3_600
_MAX_MODEL_TIMEOUT_SEC = 3_600
_MAX_OUTPUT_TOKENS = 1_000_000
_MAX_COST_MICROS = 1_000_000_000_000
_MAX_TOTAL_TOKENS = 1_000_000_000
_MIN_HARBOR_TIMEOUT_SEC = 180
_MAX_HARBOR_TIMEOUT_SEC = 86_400
_MODEL_TIMEOUT_MARGIN_SEC = 150
_CORE_TIMEOUT_MARGIN_SEC = 120
_CLI_TIMEOUT_MARGIN_SEC = 90
_CLEANUP_SERVER_TIMEOUT_SEC = 10
_CLEANUP_CLIENT_TIMEOUT_SEC = 15
_DISABLED_TOOLS = (
    "web_open",
    "pdf_read",
    "pdf_view",
    "request_user_input",
)

_INSTALL_DIR = "/installed-agent/s-code"
_PRIVATE_DIR = "/installed-agent/s-code-private"
_BUNDLE_ARCHIVE_PATH = "/installed-agent/s-code-runtime.tar.gz"
_RUNNER_PATH = "/installed-agent/s-code-harbor-runner"
_PROMPT_PATH = f"{_PRIVATE_DIR}/instruction.txt"
_PROVENANCE_SEED_PATH = f"{_PRIVATE_DIR}/provenance.seed.json"
_EXIT_STATUS_PATH = f"{_PRIVATE_DIR}/exit.status"

_EVENTS_PATH = (EnvironmentPaths.agent_dir / "events.jsonl").as_posix()
_STDERR_PATH = (EnvironmentPaths.agent_dir / "stderr.log").as_posix()
_FINAL_PATH = (EnvironmentPaths.agent_dir / "final.txt").as_posix()
_PROVENANCE_PATH = (EnvironmentPaths.agent_dir / "provenance.json").as_posix()
_DAEMON_LOG_PATH = (EnvironmentPaths.agent_dir / "daemon.log").as_posix()
_STATE_DAEMON_LOG_PATH = f"{_PRIVATE_DIR}/state/logs/daemon.log"


# The prompt is intentionally read from an uploaded file.  Harbor therefore
# logs only this fixed runner path, never task text or a credential value.  The
# S-Code CLI currently accepts its prompt positionally, so the runner performs
# the final data-to-argv handoff without invoking eval or a nested shell.
_RUNNER = f"""#!/bin/sh
set -u
umask 077

events={shlex.quote(_EVENTS_PATH)}
stderr_log={shlex.quote(_STDERR_PATH)}
final={shlex.quote(_FINAL_PATH)}
provenance={shlex.quote(_PROVENANCE_PATH)}
provenance_seed={shlex.quote(_PROVENANCE_SEED_PATH)}
prompt={shlex.quote(_PROMPT_PATH)}
exit_status={shlex.quote(_EXIT_STATUS_PATH)}
daemon_log={shlex.quote(_DAEMON_LOG_PATH)}

record_exit() {{
  recorded_status=$1
  cleanup_status=$2
  runner_status=$3
  provenance_tmp="$provenance.tmp"
  if sed \
      -e "s/\\\"daemon_cleanup_exit_code\\\": null/\\\"daemon_cleanup_exit_code\\\": $cleanup_status/" \
      -e "s/\\\"exit_code\\\": null/\\\"exit_code\\\": $recorded_status/" \
      -e "s/\\\"runner_exit_code\\\": null/\\\"runner_exit_code\\\": $runner_status/" \
      "$provenance_seed" > "$provenance_tmp"
  then
    mv "$provenance_tmp" "$provenance"
  else
    rm -f "$provenance_tmp"
  fi
  status_tmp="$exit_status.tmp"
  if printf '%s %s %s\n' \
      "$recorded_status" "$cleanup_status" "$runner_status" > "$status_tmp"
  then
    mv "$status_tmp" "$exit_status"
  else
    rm -f "$status_tmp"
  fi
}}

fail() {{
  printf '%s\n' "$1" >> "$stderr_log"
  record_exit 1 0 1
  tail -c 65536 "$stderr_log" >&2 2>/dev/null || true
  exit 1
}}

: > "$events" || exit 1
: > "$stderr_log" || exit 1
: > "$final" || fail "S-Code could not create its final-output log"
cp "$provenance_seed" "$provenance" || fail "S-Code provenance data is unavailable"

[ -f "$prompt" ] && [ ! -L "$prompt" ] || fail "S-Code prompt data is unavailable"
for directory in \
  {shlex.quote(_PRIVATE_DIR + "/home")} \
  {shlex.quote(_PRIVATE_DIR + "/state")} \
  {shlex.quote(_PRIVATE_DIR + "/run")}
do
  [ ! -e "$directory" ] && [ ! -L "$directory" ] || \
    fail "S-Code refused non-fresh trial state"
  mkdir -m 700 "$directory" || fail "S-Code could not create private trial state"
done

# Appending then removing a non-newline sentinel preserves trailing newlines in
# the uploaded instruction despite POSIX command-substitution trimming.
instruction=$(cat "$prompt"; printf x) || fail "S-Code could not read prompt data"
instruction=${{instruction%x}}

status=0
{shlex.quote(_INSTALL_DIR + "/s-code")} exec \
  --raw-events-jsonl \
  --ephemeral \
  --permission-mode full \
  --timeout "$S_CODE_HARBOR_CLI_TIMEOUT_SEC" \
  --output-last-message "$final" \
  -- "$instruction" \
  < /dev/null \
  > "$events" 2> "$stderr_log" || status=$?

cleanup_status=0
{shlex.quote(_INSTALL_DIR + "/s-code")} stop \
  >> "$stderr_log" 2>&1 || cleanup_status=$?

# Keep the daemon's own log with the trial; it records why model requests failed.
cp {shlex.quote(_STATE_DAEMON_LOG_PATH)} "$daemon_log" 2>/dev/null || true

runner_status=$status
record_exit "$status" "$cleanup_status" "$runner_status"
chmod 600 "$events" "$stderr_log" "$final" "$provenance" "$daemon_log" 2>/dev/null || true

if [ "$runner_status" -ne 0 ]; then
  # Feed only S-Code's own bounded stderr tail back to Harbor so
  # BaseInstalledAgent can classify the non-zero exit correctly.  The complete
  # stderr remains in /logs/agent/stderr.log.
  tail -c 65536 "$stderr_log" >&2 2>/dev/null || true
fi
exit "$runner_status"
"""


def _positive_bounded_int(
    value: object,
    *,
    name: str,
    maximum: int,
    minimum: int = 1,
) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise TypeError(f"{name} must be an integer")
    if not minimum <= value <= maximum:
        raise ValueError(f"{name} must be between {minimum} and {maximum}")
    return value


def _optional_temperature(value: object) -> float | None:
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise TypeError("temperature must be a number")
    if not math.isfinite(value) or not 0 <= value <= 2:
        raise ValueError("temperature must be between 0 and 2")
    return float(value)


def _model_base_url(value: object) -> str:
    if not isinstance(value, str) or not value or value != value.strip():
        raise ValueError("model_base_url must be a non-empty absolute URL")
    if any(character.isspace() for character in value):
        raise ValueError("model_base_url must not contain whitespace")
    parsed = urlsplit(value)
    if parsed.scheme not in {"http", "https"} or not parsed.hostname:
        raise ValueError("model_base_url must be an absolute HTTP(S) URL")
    try:
        parsed.port
    except ValueError as exc:
        raise ValueError("model_base_url has an invalid port") from exc
    if parsed.username is not None or parsed.password is not None:
        raise ValueError("model_base_url must not contain credentials")
    if parsed.query or parsed.fragment:
        raise ValueError("model_base_url must not contain a query or fragment")
    loopback = parsed.hostname.casefold() == "localhost"
    if not loopback:
        try:
            loopback = ip_address(parsed.hostname).is_loopback
        except ValueError:
            pass
    if parsed.scheme != "https" and not loopback:
        raise ValueError("model_base_url must use HTTPS or loopback HTTP")
    return value


def _model_name(value: object) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError("model_name is required")
    if value != value.strip() or "\x00" in value or "\n" in value or "\r" in value:
        raise ValueError("model_name must be a single non-empty identifier")
    return value


def _credential_env(value: object) -> str:
    if not isinstance(value, str) or not _CREDENTIAL_ENV_RE.fullmatch(value):
        raise ValueError("credential_env must be an uppercase environment-variable name")
    if _SENSITIVE_ENV_TOKEN_RE.search(value) is None:
        raise ValueError(
            "credential_env must contain KEY, SECRET, TOKEN, PASSWORD, "
            "CREDENTIAL, or AUTH so Harbor treats its value as sensitive"
        )
    return value


def _harbor_version() -> str:
    actual = distribution_version("harbor")
    if actual != _SUPPORTED_HARBOR_VERSION:
        raise RuntimeError(
            "S-Code's Harbor adapter requires harbor=="
            f"{_SUPPORTED_HARBOR_VERSION}; found {actual}"
        )
    return actual


def _resolved_harbor_agent_timeout_sec(environment: BaseEnvironment) -> int:
    """Reproduce Harbor 0.17's effective single-step agent deadline."""

    trial_path = environment.trial_paths.config_path
    task_path = environment.environment_dir.parent / "task.toml"
    try:
        trial = TrialConfig.model_validate_json(trial_path.read_text(encoding="utf-8"))
        task = HarborTaskConfig.model_validate_toml(
            task_path.read_text(encoding="utf-8")
        )
    except (OSError, UnicodeError, ValueError) as exc:
        raise RuntimeError("could not resolve Harbor's agent deadline") from exc
    if task.steps:
        raise ValueError("SCodeAgent does not support multi-step Harbor tasks")

    base = trial.agent.override_timeout_sec or task.agent.timeout_sec
    if base is None:
        raise ValueError("Harbor task must define an agent timeout")
    maximum = trial.agent.max_timeout_sec or float("inf")
    multiplier = (
        trial.agent_timeout_multiplier
        if trial.agent_timeout_multiplier is not None
        else trial.timeout_multiplier
    )
    resolved = min(base, maximum) * multiplier
    if not math.isfinite(resolved):
        raise ValueError("Harbor resolved a non-finite agent timeout")
    timeout = math.floor(resolved)
    if not _MIN_HARBOR_TIMEOUT_SEC <= timeout <= _MAX_HARBOR_TIMEOUT_SEC:
        raise ValueError(
            "Harbor's resolved agent timeout must be between "
            f"{_MIN_HARBOR_TIMEOUT_SEC} and {_MAX_HARBOR_TIMEOUT_SEC} seconds"
        )
    return timeout


def _runtime_preflight_command(required_glibc: str) -> str:
    """Return a POSIX-shell preflight with numeric dotted-version comparison."""

    required = required_glibc.removeprefix("glibc ")
    return f"""set -eu
for required_command in \
  cat chmod chown cp getconf gzip head id mkdir mv rm sed sha256sum sleep stat tail tar uname wc
do
  command -v "$required_command" >/dev/null || {{
    echo "S-Code requires command: $required_command" >&2
    exit 1
  }}
done
[ -x /usr/bin/nohup ] || [ -x /bin/nohup ] || {{
  echo 'S-Code requires /usr/bin/nohup or /bin/nohup' >&2
  exit 1
}}
[ "$(uname -s)" = Linux ] || {{ echo 'S-Code requires Linux' >&2; exit 1; }}
[ "$(uname -m)" = x86_64 ] || {{ echo 'S-Code requires x86_64' >&2; exit 1; }}
actual_glibc=$(getconf GNU_LIBC_VERSION 2>/dev/null) || {{
  echo 'S-Code requires detectable glibc' >&2
  exit 1
}}
case "$actual_glibc" in
  'glibc '*) actual_glibc=${{actual_glibc#glibc }} ;;
  *) echo 'S-Code requires detectable glibc' >&2; exit 1 ;;
esac
version_at_least() {{
  actual_left=$1
  required_left=$2
  while [ -n "$actual_left$required_left" ]; do
    case "$actual_left" in
      *.*) actual_part=${{actual_left%%.*}}; actual_left=${{actual_left#*.}} ;;
      '') actual_part=0 ;;
      *) actual_part=$actual_left; actual_left= ;;
    esac
    case "$required_left" in
      *.*) required_part=${{required_left%%.*}}; required_left=${{required_left#*.}} ;;
      '') required_part=0 ;;
      *) required_part=$required_left; required_left= ;;
    esac
    case "$actual_part:$required_part" in
      *[!0-9:]*) return 2 ;;
    esac
    [ "$actual_part" -gt "$required_part" ] && return 0
    [ "$actual_part" -lt "$required_part" ] && return 1
  done
  return 0
}}
if ! version_at_least "$actual_glibc" {shlex.quote(required)}; then
  echo "S-Code bundle requires glibc >= {required}; found $actual_glibc" >&2
  exit 1
fi
test ! -e {shlex.quote(_INSTALL_DIR)}
test ! -L {shlex.quote(_INSTALL_DIR)}
test ! -e {shlex.quote(_PRIVATE_DIR)}
test ! -L {shlex.quote(_PRIVATE_DIR)}
test ! -e {shlex.quote(_BUNDLE_ARCHIVE_PATH)}
test ! -L {shlex.quote(_BUNDLE_ARCHIVE_PATH)}
test ! -e {shlex.quote(_RUNNER_PATH)}
test ! -L {shlex.quote(_RUNNER_PATH)}
mkdir -m 0755 {shlex.quote(_INSTALL_DIR)}
mkdir -m 0700 {shlex.quote(_PRIVATE_DIR)}
"""


def _fallback_terminal(exit_code: int | None) -> tuple[str, str | None]:
    if exit_code == 0:
        return "failed", None
    if exit_code == 124:
        return "timed_out", "cli_timeout"
    if exit_code == 130:
        return "cancelled", None
    return "failed", None


def _classify_event_stream(path: Path, exit_code: int | None) -> dict[str, Any]:
    """Classify a completed run from S-Code's JSONL terminal evidence."""

    status, limit_kind = _fallback_terminal(exit_code)
    failure_kind: str | None = None
    terminal: dict[str, Any] | None = None
    usage: dict[str, int] = {}
    model_requests = {"accepted": 0, "rejected": 0, "connection_error": 0}
    records = 0
    invalid_line: int | None = None

    try:
        source = path.open("rb")
    except OSError:
        return {
            "status": status,
            "limit_kind": limit_kind,
            "failure_kind": failure_kind,
            "terminal_event": None,
            "usage": {},
            "model_requests": model_requests,
            "event_stream": {
                "validation": "unavailable",
                "records": 0,
                "error": "events.jsonl could not be read",
            },
        }

    with source:
        for line_number, line in enumerate(source, start=1):
            if not line.strip():
                invalid_line = invalid_line or line_number
                continue
            try:
                event = json.loads(line)
            except (json.JSONDecodeError, UnicodeError):
                invalid_line = invalid_line or line_number
                continue
            if not isinstance(event, dict):
                invalid_line = invalid_line or line_number
                continue
            records += 1
            kind = event.get("kind") or event.get("type")
            if not isinstance(kind, str):
                continue
            payload = event.get("payload")
            if not isinstance(payload, dict):
                payload = {}
            error_code = event.get("error_code") or payload.get("error_code")
            if not isinstance(error_code, str):
                error_code = None
            reason = payload.get("reason")
            if not isinstance(reason, str):
                reason = None
            if kind == "privacy.request.finished":
                request = payload.get("request")
                outcome = request.get("status") if isinstance(request, dict) else None
                if outcome in model_requests:
                    model_requests[outcome] += 1

            usage_keys = (
                ("input_units", "input_tokens"),
                ("output_units", "output_tokens"),
                ("input_tokens", "input_tokens"),
                ("output_tokens", "output_tokens"),
                ("model_calls", "model_calls"),
                ("tool_calls", "tool_calls"),
            )
            if kind == "turn.usage" or kind.startswith("turn."):
                for source_key, target_key in usage_keys:
                    value = payload.get(source_key)
                    if isinstance(value, int) and not isinstance(value, bool) and value >= 0:
                        usage[target_key] = value

            candidate: tuple[str, str | None] | None = None
            candidate_failure: str | None = None
            if kind == "question.required" and payload.get("source") == "tool_call_limit":
                error_code = "tool_call_limit"
                candidate = ("limit_stopped", "tool_call_limit")
            elif kind == "turn.awaiting_input" and error_code == "tool_call_limit":
                candidate = ("limit_stopped", "tool_call_limit")
            elif kind in {"turn.awaiting_input", "question.required"}:
                error_code = error_code or "input_required"
                candidate = ("failed", None)
                candidate_failure = "input_required"
            elif kind in {"turn.awaiting_approval", "approval.required"}:
                error_code = error_code or "approval_required"
                candidate = ("failed", None)
                candidate_failure = "approval_required"
            elif kind == "turn.completed":
                candidate = ("completed", None)
            elif kind == "turn.cancelled":
                candidate = ("cancelled", None)
            elif kind == "turn.failed":
                if error_code == "tool_call_limit":
                    candidate = ("limit_stopped", "tool_call_limit")
                elif error_code == "model_stream_idle_timeout":
                    candidate = ("timed_out", "model_stream_idle_timeout")
                elif error_code == "turn_elapsed_timeout":
                    candidate = ("timed_out", "turn_elapsed_timeout")
                elif reason == "model-call limit reached":
                    candidate = ("limit_stopped", "model_call_limit")
                else:
                    candidate = ("failed", None)

            if candidate is not None:
                status, limit_kind = candidate
                failure_kind = candidate_failure
                terminal = {
                    "line": line_number,
                    "kind": kind,
                    "error_code": error_code,
                }
                sequence = event.get("sequence")
                if isinstance(sequence, int) and not isinstance(sequence, bool):
                    terminal["sequence"] = sequence

    # A run that never received a model response measures the endpoint, not
    # S-Code, so name the cause and let run() report it as infrastructure.
    if status != "completed" and failure_kind is None and model_requests["accepted"] == 0:
        if model_requests["connection_error"]:
            failure_kind = "model_unreachable"
        elif model_requests["rejected"]:
            failure_kind = "model_rejected"

    if invalid_line is not None:
        validation = "invalid"
        error = f"invalid JSONL record at line {invalid_line}"
    elif terminal is None:
        validation = "incomplete"
        error = "no terminal event was found"
    else:
        validation = "valid"
        error = None
    return {
        "status": status,
        "limit_kind": limit_kind,
        "failure_kind": failure_kind,
        "terminal_event": terminal,
        "usage": usage,
        "model_requests": model_requests,
        "event_stream": {
            "validation": validation,
            "records": records,
            "error": error,
        },
    }


def _parse_exit_status(
    path: Path,
) -> tuple[int | None, int | None, int | None]:
    try:
        if path.stat().st_size > 32:
            return None, None, None
        value = path.read_text(encoding="ascii")
    except (OSError, UnicodeError):
        return None, None, None
    match = re.fullmatch(
        r"([0-9]{1,3}) ([0-9]{1,3}) ([0-9]{1,3})\n?",
        value,
    )
    if match is None:
        return None, None, None
    exit_code, cleanup_code, runner_code = (int(part) for part in match.groups())
    if exit_code > 255 or cleanup_code > 255 or runner_code > 255:
        return None, None, None
    return exit_code, cleanup_code, runner_code


def _manifest_provenance(manifest: BundleManifest) -> dict[str, Any]:
    return {
        "schema_version": manifest.schema_version,
        "bundle_format": manifest.bundle_format,
        "source_commit": manifest.source_commit,
        "source_tree": manifest.source_tree,
        "source_dirty": manifest.source_dirty,
        "target": manifest.target,
        "glibc_version": manifest.glibc_version,
        "lock_sha256s": dict(manifest.lock_sha256s),
        "tool_versions": dict(manifest.tool_versions),
        "files": {
            name: {
                "sha256": metadata.sha256,
                "size": metadata.size,
                "mode": f"{metadata.mode:04o}",
            }
            for name, metadata in manifest.files.items()
        },
        "archive_sha256": manifest.archive_sha256,
    }


async def _upload_text(
    environment: BaseEnvironment,
    contents: str,
    target_path: str,
    *,
    mode: int,
) -> None:
    with tempfile.TemporaryDirectory(prefix="s-code-harbor-upload-") as directory:
        source = Path(directory) / "payload"
        # Encode explicitly so a Windows Harbor host cannot translate LF to
        # CRLF and alter the prompt, JSON evidence, or POSIX runner bytes.
        source.write_bytes(contents.encode("utf-8"))
        source.chmod(mode)
        await environment.upload_file(source, target_path)


class SCodeAgent(BaseInstalledAgent):
    """Run exactly one isolated, non-interactive S-Code turn in Harbor."""

    SUPPORTS_ATIF = False
    SUPPORTS_WINDOWS = False

    def __init__(
        self,
        logs_dir: Path,
        model_name: str | None = None,
        *,
        bundle_path: str | Path,
        bundle_sha256: str,
        model_base_url: str,
        credential_env: str,
        max_model_calls: int = 100,
        max_tool_calls: int = 256,
        model_stream_idle_seconds: int = 300,
        model_connect_timeout_sec: int = 10,
        model_read_timeout_sec: int = 600,
        model_request_timeout_sec: int = 3_600,
        max_cost_micros: int = 5_000_000,
        max_total_tokens: int = 10_000_000,
        temperature: float | None = None,
        max_output_tokens: int | None = None,
        extra_env: dict[str, str] | None = None,
        **kwargs: Any,
    ) -> None:
        self._harbor_version = _harbor_version()
        self._bundle_path = Path(bundle_path).expanduser().resolve(strict=True)
        if not self._bundle_path.is_file():
            raise ValueError("bundle_path must name a local regular file")
        self._bundle_sha256 = validate_sha256(bundle_sha256, field="bundle_sha256")
        self._model_base_url = _model_base_url(model_base_url)
        self._credential_env = _credential_env(credential_env)
        self._model_name = _model_name(model_name)
        self._max_model_calls = _positive_bounded_int(
            max_model_calls, name="max_model_calls", maximum=_MAX_CALL_LIMIT
        )
        self._max_tool_calls = _positive_bounded_int(
            max_tool_calls, name="max_tool_calls", maximum=_MAX_CALL_LIMIT
        )
        self._requested_model_stream_idle_seconds = _positive_bounded_int(
            model_stream_idle_seconds,
            name="model_stream_idle_seconds",
            maximum=_MAX_IDLE_SEC,
        )
        # Self-hosted models can stream slowly and go quiet while vLLM buffers
        # array-valued tool arguments, so the gateway's interactive defaults
        # (45 s between bytes, 180 s per request) are too short here.
        self._requested_model_connect_timeout_sec = _positive_bounded_int(
            model_connect_timeout_sec,
            name="model_connect_timeout_sec",
            maximum=_MAX_MODEL_TIMEOUT_SEC,
        )
        self._requested_model_read_timeout_sec = _positive_bounded_int(
            model_read_timeout_sec,
            name="model_read_timeout_sec",
            maximum=_MAX_MODEL_TIMEOUT_SEC,
        )
        self._requested_model_request_timeout_sec = _positive_bounded_int(
            model_request_timeout_sec,
            name="model_request_timeout_sec",
            maximum=_MAX_MODEL_TIMEOUT_SEC,
        )
        self._max_cost_micros = _positive_bounded_int(
            max_cost_micros,
            name="max_cost_micros",
            maximum=_MAX_COST_MICROS,
        )
        self._max_total_tokens = _positive_bounded_int(
            max_total_tokens,
            name="max_total_tokens",
            maximum=_MAX_TOTAL_TOKENS,
        )
        # Unset keeps S-Code's own sampling; benchmarks set these to match a baseline.
        self._temperature = _optional_temperature(temperature)
        self._max_output_tokens = (
            None
            if max_output_tokens is None
            else _positive_bounded_int(
                max_output_tokens,
                name="max_output_tokens",
                maximum=_MAX_OUTPUT_TOKENS,
            )
        )
        # Harbor's real per-task deadline becomes available through the
        # environment in install().  These placeholder values keep setup
        # deterministic; install() replaces them before the run phase.
        self._set_effective_timeouts(3_600)

        supplied_env = dict(extra_env or {})
        credential = supplied_env.get(self._credential_env)
        if not isinstance(credential, str) or not credential:
            raise ValueError(
                "credential_env must name a non-empty value supplied through "
                "Harbor's agent env/extra_env"
            )
        managed_env = {
            "S_CODE_MODEL_PROVIDER": "openai_compatible",
            "S_CODE_MODEL_BASE_URL": self._model_base_url,
            "S_CODE_MODEL_CREDENTIAL_HANDLE": self._credential_env,
            "S_CODE_MODEL": self._model_name,
            "S_CODE_MODEL_CONNECT_TIMEOUT_SECONDS": str(
                self._model_connect_timeout_sec
            ),
            "S_CODE_MODEL_READ_TIMEOUT_SECONDS": str(self._model_read_timeout_sec),
            "S_CODE_MODEL_REQUEST_TIMEOUT_SECONDS": str(
                self._model_request_timeout_sec
            ),
            # Qwen3.6's chat template, like others, rejects later system messages.
            "S_CODE_MODEL_MERGE_SYSTEM_MESSAGES": "true",
            "S_CODE_HOME": f"{_PRIVATE_DIR}/home",
            "S_CODE_STATE_DIR": f"{_PRIVATE_DIR}/state",
            "S_CODE_RUNTIME_DIR": f"{_PRIVATE_DIR}/run",
            "S_CODE_TURN_MAX_MODEL_CALLS": str(self._max_model_calls),
            "S_CODE_TURN_MAX_TOOL_CALLS": str(self._max_tool_calls),
            "S_CODE_TURN_MAX_ELAPSED_SECONDS": str(self._core_timeout_sec),
            "S_CODE_TURN_MODEL_STREAM_IDLE_SECONDS": str(
                self._model_stream_idle_seconds
            ),
            "S_CODE_TURN_MAX_COST_MICROS": str(self._max_cost_micros),
            "S_CODE_TURN_MAX_TOTAL_TOKENS": str(self._max_total_tokens),
            "S_CODE_TURN_DISABLED_TOOLS": ",".join(_DISABLED_TOOLS),
            "S_CODE_HARBOR_CLI_TIMEOUT_SEC": str(self._cli_timeout_sec),
            "NO_COLOR": "1",
            "TERM": "dumb",
        }
        if self._temperature is not None:
            managed_env["S_CODE_MODEL_TEMPERATURE"] = str(self._temperature)
        if self._max_output_tokens is not None:
            managed_env["S_CODE_MODEL_MAX_OUTPUT_TOKENS"] = str(self._max_output_tokens)
        for name, expected in managed_env.items():
            if name in supplied_env and supplied_env[name] != expected:
                raise ValueError(f"{name} is managed by SCodeAgent and cannot be overridden")
        supplied_env.update(managed_env)

        # Validation is repeated immediately before upload in install(); doing it
        # here makes configuration errors fail before Harbor starts a container.
        self._manifest = validate_bundle(self._bundle_path, self._bundle_sha256)
        self._agent_owner: str | None = None
        self._has_run = False
        super().__init__(
            logs_dir=logs_dir,
            model_name=self._model_name,
            version=_ADAPTER_VERSION,
            extra_env=supplied_env,
            **kwargs,
        )

    def _set_effective_timeouts(self, harbor_timeout_sec: int) -> None:
        self._harbor_timeout_sec = harbor_timeout_sec
        self._model_request_timeout_sec = min(
            self._requested_model_request_timeout_sec,
            harbor_timeout_sec - _MODEL_TIMEOUT_MARGIN_SEC,
        )
        self._model_connect_timeout_sec = min(
            self._requested_model_connect_timeout_sec,
            self._model_request_timeout_sec,
        )
        self._model_read_timeout_sec = min(
            self._requested_model_read_timeout_sec,
            self._model_request_timeout_sec,
        )
        self._model_stream_idle_seconds = min(
            self._requested_model_stream_idle_seconds,
            self._model_request_timeout_sec,
        )
        self._core_timeout_sec = harbor_timeout_sec - _CORE_TIMEOUT_MARGIN_SEC
        self._cli_timeout_sec = harbor_timeout_sec - _CLI_TIMEOUT_MARGIN_SEC

    def _effective_timeout_env(self) -> dict[str, str]:
        return {
            "S_CODE_MODEL_CONNECT_TIMEOUT_SECONDS": str(
                self._model_connect_timeout_sec
            ),
            "S_CODE_MODEL_READ_TIMEOUT_SECONDS": str(self._model_read_timeout_sec),
            "S_CODE_MODEL_REQUEST_TIMEOUT_SECONDS": str(
                self._model_request_timeout_sec
            ),
            "S_CODE_TURN_MAX_ELAPSED_SECONDS": str(self._core_timeout_sec),
            "S_CODE_TURN_MODEL_STREAM_IDLE_SECONDS": str(
                self._model_stream_idle_seconds
            ),
            "S_CODE_HARBOR_CLI_TIMEOUT_SEC": str(self._cli_timeout_sec),
        }

    @staticmethod
    @override
    def name() -> str:
        return "s-code"

    async def _resolve_agent_owner(self, environment: BaseEnvironment) -> None:
        result = await self.exec_as_agent(
            environment,
            command="command -v id >/dev/null && id -u && id -g",
        )
        lines = (result.stdout or "").splitlines()
        if len(lines) != 2 or any(re.fullmatch(r"[0-9]+", line) is None for line in lines):
            raise RuntimeError("could not resolve the Harbor agent user's numeric uid:gid")
        uid, gid = (int(line) for line in lines)
        if uid > 4_294_967_295 or gid > 4_294_967_295:
            raise RuntimeError("Harbor agent uid:gid is outside the supported range")
        self._agent_owner = f"{uid}:{gid}"

    def _owner_command(self, *paths: str) -> str:
        if self._agent_owner is None:
            raise RuntimeError("Harbor agent ownership was not resolved")
        owner = shlex.quote(self._agent_owner)
        quoted_paths = " ".join(shlex.quote(path) for path in paths)
        return f"chown -- {owner} {quoted_paths}"

    @override
    async def install(self, environment: BaseEnvironment) -> None:
        self._set_effective_timeouts(_resolved_harbor_agent_timeout_sec(environment))
        self._extra_env.update(self._effective_timeout_env())
        await self._resolve_agent_owner(environment)
        await self.exec_as_root(
            environment,
            command=_runtime_preflight_command(self._manifest.glibc_version),
        )

        with tempfile.TemporaryDirectory(prefix="s-code-harbor-bundle-") as directory:
            # Authenticate, validate, and upload the same private snapshot.  A
            # concurrent replacement of the configured path cannot create a
            # validation/upload race.
            snapshot = Path(directory) / "bundle.tar.gz"
            shutil.copyfile(self._bundle_path, snapshot)
            current_manifest = validate_bundle(snapshot, self._bundle_sha256)
            if current_manifest != self._manifest:
                raise ValueError("bundle changed after adapter construction")
            await environment.upload_file(snapshot, _BUNDLE_ARCHIVE_PATH)

        await _upload_text(environment, _RUNNER, _RUNNER_PATH, mode=0o755)
        owner_private = self._owner_command(_PRIVATE_DIR)
        executables = " ".join(
            shlex.quote(f"{_INSTALL_DIR}/{name}") for name in _EXECUTABLE_MEMBERS
        )
        await self.exec_as_root(
            environment,
            command=(
                "set -eu; "
                f"printf '%s  %s\\n' {shlex.quote(self._bundle_sha256)} "
                f"{shlex.quote(_BUNDLE_ARCHIVE_PATH)} | sha256sum -c - >/dev/null; "
                f"tar -xzf {shlex.quote(_BUNDLE_ARCHIVE_PATH)} "
                f"-C {shlex.quote(_INSTALL_DIR)}; "
                f"rm -f {shlex.quote(_BUNDLE_ARCHIVE_PATH)}; "
                f"chmod 0644 {shlex.quote(_INSTALL_DIR + '/manifest.json')}; "
                f"chmod 0755 {executables}; "
                f"chmod 0755 {shlex.quote(_RUNNER_PATH)}; "
                f"chmod 0700 {shlex.quote(_PRIVATE_DIR)}; "
                f"{owner_private}; "
                f"{shlex.quote(_INSTALL_DIR + '/s-code')} --help >/dev/null; "
                f"{shlex.quote(_INSTALL_DIR + '/s-code-cli')} --self-test >/dev/null; "
                f"{shlex.quote(_INSTALL_DIR + '/s-code-daemon')} --self-test >/dev/null; "
                f"{shlex.quote(_INSTALL_DIR + '/s-code-pdf-worker')} --self-test >/dev/null"
            ),
        )

    def _provenance(self) -> dict[str, Any]:
        return {
            "schema_version": "1",
            "adapter": {
                "name": self.name(),
                "version": _ADAPTER_VERSION,
                "harbor_version": self._harbor_version,
                "supports_atif": False,
            },
            "bundle": _manifest_provenance(self._manifest),
            "model": {
                "provider": "openai_compatible",
                "name": self._model_name,
                "base_url": self._model_base_url,
                "credential_env": self._credential_env,
            },
            "execution": {
                "attempts": 1,
                "ephemeral": True,
                "permission_mode": "full",
                "output_mode": "raw_events_jsonl",
                "max_model_calls": self._max_model_calls,
                "max_tool_calls": self._max_tool_calls,
                "disabled_tools": list(_DISABLED_TOOLS),
                "model_stream_idle_seconds": self._model_stream_idle_seconds,
                "model_connect_timeout_sec": self._model_connect_timeout_sec,
                "model_read_timeout_sec": self._model_read_timeout_sec,
                "model_request_timeout_sec": self._model_request_timeout_sec,
                "temperature": self._temperature,
                "max_output_tokens": self._max_output_tokens,
                "max_cost_micros": self._max_cost_micros,
                "cost_metering_available": False,
                "max_total_tokens": self._max_total_tokens,
                "core_timeout_sec": self._core_timeout_sec,
                "cli_timeout_sec": self._cli_timeout_sec,
                "harbor_timeout_sec": self._harbor_timeout_sec,
                "daemon_cleanup_exit_code": None,
                "exit_code": None,
                "post_run_cleanup_exit_code": None,
                "post_run_cleanup_status": "pending",
                "runner_exit_code": None,
            },
            "result": {
                "status": "pending",
                "limit_kind": None,
                "failure_kind": None,
                "terminal_event": None,
                "event_stream": {
                    "validation": "pending",
                    "records": 0,
                    "error": None,
                },
            },
            "artifacts": {
                "events": _EVENTS_PATH,
                "stderr": _STDERR_PATH,
                "final": _FINAL_PATH,
                "provenance": _PROVENANCE_PATH,
                "daemon_log": _DAEMON_LOG_PATH,
            },
        }

    async def _finalize_provenance(
        self,
        environment: BaseEnvironment,
        context: AgentContext,
        *,
        process_succeeded: bool,
        post_cleanup_code: int | None,
        post_cleanup_status: str,
    ) -> dict[str, Any]:
        with tempfile.TemporaryDirectory(prefix="s-code-harbor-results-") as directory:
            local = Path(directory)
            status_path = local / "exit.status"
            events_path = local / "events.jsonl"

            try:
                await environment.download_file(_EXIT_STATUS_PATH, status_path)
            except Exception:
                exit_code, cleanup_code, runner_code = (
                    (0, None, 0) if process_succeeded else (None, None, None)
                )
                status_validation = "unavailable"
            else:
                recorded_exit, cleanup_code, runner_code = _parse_exit_status(status_path)
                if recorded_exit is None or runner_code is None:
                    exit_code, runner_code = (
                        (0, 0) if process_succeeded else (None, None)
                    )
                    status_validation = "invalid"
                elif (
                    recorded_exit != runner_code
                    or process_succeeded != (runner_code == 0)
                ):
                    exit_code = recorded_exit
                    status_validation = "inconsistent"
                else:
                    exit_code = recorded_exit
                    status_validation = "valid"

            try:
                await environment.download_file(_EVENTS_PATH, events_path)
            except Exception:
                pass
            result = _classify_event_stream(events_path, exit_code)
            result["process_status_validation"] = status_validation
            usage = result["usage"]
            if "input_tokens" in usage:
                context.n_input_tokens = usage["input_tokens"]
            if "output_tokens" in usage:
                context.n_output_tokens = usage["output_tokens"]
            if context.metadata is not None:
                context.metadata["s_code"]["result_status"] = result["status"]
                context.metadata["s_code"]["usage"] = usage

            provenance = self._provenance()
            provenance["execution"]["exit_code"] = exit_code
            provenance["execution"]["daemon_cleanup_exit_code"] = cleanup_code
            provenance["execution"]["post_run_cleanup_exit_code"] = post_cleanup_code
            provenance["execution"]["post_run_cleanup_status"] = post_cleanup_status
            provenance["execution"]["runner_exit_code"] = runner_code
            provenance["result"] = result
            contents = json.dumps(
                provenance,
                ensure_ascii=False,
                indent=2,
                sort_keys=True,
            ) + "\n"
            await _upload_text(
                environment,
                contents,
                _PROVENANCE_PATH,
                mode=0o600,
            )
            return result

    async def _bounded_cancel_cleanup(self, environment: BaseEnvironment) -> None:
        command = (
            "status=0; "
            f"{shlex.quote(_INSTALL_DIR + '/s-code')} stop >/dev/null 2>&1 || status=$?; "
            f"cp {shlex.quote(_STATE_DAEMON_LOG_PATH)} "
            f"{shlex.quote(_DAEMON_LOG_PATH)} 2>/dev/null || true; "
            f"chmod 600 {shlex.quote(_DAEMON_LOG_PATH)} 2>/dev/null || true; "
            'exit "$status"'
        )
        try:
            await asyncio.wait_for(
                environment.exec(
                    command=command,
                    timeout_sec=_CLEANUP_SERVER_TIMEOUT_SEC,
                ),
                timeout=_CLEANUP_CLIENT_TIMEOUT_SEC,
            )
        except BaseException:
            # The original Harbor cancellation remains authoritative.
            pass

    @with_prompt_template
    @override
    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        if self._has_run:
            raise RuntimeError("SCodeAgent supports exactly one run per trial")
        self._has_run = True
        if not isinstance(instruction, str) or not instruction or "\x00" in instruction:
            raise ValueError("instruction must be non-empty text without NUL bytes")

        provenance = json.dumps(
            self._provenance(),
            ensure_ascii=False,
            indent=2,
            sort_keys=True,
        ) + "\n"
        await _upload_text(
            environment,
            instruction,
            _PROMPT_PATH,
            mode=0o600,
        )
        await _upload_text(
            environment,
            provenance,
            _PROVENANCE_SEED_PATH,
            mode=0o600,
        )
        owner_inputs = self._owner_command(
            _PROMPT_PATH,
            _PROVENANCE_SEED_PATH,
        )
        await self.exec_as_root(
            environment,
            command=(
                f"chmod 0600 {shlex.quote(_PROMPT_PATH)} "
                f"{shlex.quote(_PROVENANCE_SEED_PATH)}; "
                f"{owner_inputs}"
            ),
        )

        context.metadata = {
            "s_code": {
                "events": _EVENTS_PATH,
                "stderr": _STDERR_PATH,
                "final": _FINAL_PATH,
                "provenance": _PROVENANCE_PATH,
                "daemon_log": _DAEMON_LOG_PATH,
                "supports_atif": False,
            }
        }

        execution_error: BaseException | None = None
        try:
            await self.exec_as_agent(
                environment,
                command=shlex.quote(_RUNNER_PATH),
            )
        except BaseException as error:
            execution_error = error

        if isinstance(execution_error, asyncio.CancelledError):
            await self._bounded_cancel_cleanup(environment)
            raise execution_error
        if execution_error is not None and not isinstance(execution_error, Exception):
            raise execution_error

        post_cleanup_error: Exception | None = None
        post_cleanup_code: int | None = None
        try:
            cleanup_result = await asyncio.wait_for(
                environment.exec(
                    command=f"{shlex.quote(_INSTALL_DIR + '/s-code')} stop",
                    timeout_sec=_CLEANUP_SERVER_TIMEOUT_SEC,
                ),
                timeout=_CLEANUP_CLIENT_TIMEOUT_SEC,
            )
            post_cleanup_code = cleanup_result.return_code
            post_cleanup_status = (
                "succeeded" if post_cleanup_code == 0 else "failed"
            )
        except asyncio.CancelledError:
            # A deadline or cancellation during cleanup stays a cancellation, so
            # Harbor still records it as one.
            await self._bounded_cancel_cleanup(environment)
            raise
        except Exception as error:
            post_cleanup_error = error
            post_cleanup_status = "error"

        finalized_result: dict[str, Any] | None = None
        finalization_error: Exception | None = None
        try:
            finalized_result = await self._finalize_provenance(
                environment,
                context,
                process_succeeded=execution_error is None,
                post_cleanup_code=post_cleanup_code,
                post_cleanup_status=post_cleanup_status,
            )
        except Exception as error:
            finalization_error = error
            self.logger.error("S-Code could not finalize Harbor provenance")

        if post_cleanup_error is not None:
            raise NonZeroAgentExitCodeError(
                "S-Code daemon cleanup could not confirm quiescence"
            ) from post_cleanup_error
        if post_cleanup_code != 0:
            raise NonZeroAgentExitCodeError(
                "S-Code daemon cleanup failed; task state is not quiescent"
            )
        if finalized_result is not None and (
            finalized_result["process_status_validation"] == "valid"
            and finalized_result["event_stream"]["validation"] == "valid"
        ):
            failure_kind = finalized_result["failure_kind"]
            if failure_kind == "model_unreachable":
                raise NetworkConnectionError(
                    "S-Code did not receive a response from the model endpoint"
                ) from execution_error
            if failure_kind == "model_rejected":
                raise UnknownApiError(
                    "The model endpoint rejected every S-Code request"
                ) from execution_error
        if (
            finalized_result is not None
            and finalized_result["status"] == "timed_out"
            and finalized_result["limit_kind"] in {"turn_elapsed_timeout", "cli_timeout"}
        ):
            # Harbor records a TimeoutError from run() as AgentTimeoutError, still
            # runs the verifier, and does not retry it by default.
            raise TimeoutError("S-Code stopped at its time limit") from execution_error
        if execution_error is not None:
            raise execution_error
        if finalization_error is not None:
            raise NonZeroAgentExitCodeError(
                "S-Code could not validate and preserve its completed run"
            ) from finalization_error
        if finalized_result is None or (
            finalized_result["process_status_validation"] != "valid"
            or finalized_result["event_stream"]["validation"] != "valid"
            or finalized_result["status"] != "completed"
        ):
            raise NonZeroAgentExitCodeError(
                "S-Code reported success without valid completed-run evidence"
            )
