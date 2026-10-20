# S-Code adapter for Harbor

This adapter runs one noninteractive S-Code attempt in a Harbor-managed task
container. It targets Harbor 0.17.0 so benchmark runs use a known custom-agent
API and a checksum-verified S-Code runtime.

## Requirements

- Python 3.12 or newer with `harbor==0.17.0` from `requirements.txt`.
- An x86-64 glibc Linux build host and task container.
- A clean S-Code checkout with the pinned Rust 1.89.0 and Node 22.23.2 tools.
- An OpenAI-compatible model endpoint reachable from the task container.

The Linux restriction applies to this Harbor bundle, not to S-Code generally.

Install the pinned Harbor version in a dedicated environment. Keep it under the
git-ignored `.work/` directory so the checkout stays clean for the bundle build:

```sh
python3.12 -m venv .work/harbor-venv
. .work/harbor-venv/bin/activate
python -m pip install -r integrations/harbor/requirements.txt
```

## Build the runtime bundle

From the repository root on x86-64 glibc Linux, run:

```sh
integrations/harbor/build-bundle.sh
```

The builder refuses a dirty checkout, builds from the checked-out commit, runs
the runtime self-tests, and writes these files under `.work/harbor`:

```text
s-code-<commit>-x86_64-unknown-linux-gnu.tar.gz
s-code-<commit>-x86_64-unknown-linux-gnu.tar.gz.sha256
```

Set `S_CODE_HARBOR_OUTPUT_DIR` to choose another output directory and
`CARGO_TARGET_DIR` to reuse a Cargo build directory. The archive contains only
`manifest.json`, `s-code`, `s-code-cli`, `s-code-daemon`, and
`s-code-pdf-worker`.

## Run a Harbor task

Run Harbor from the repository root with the repository on `PYTHONPATH`, so
Harbor can import the adapter, and replace the placeholders below. Write job
output under `.work/` too, so the checkout stays clean.

S-Code calls the model from inside the task container, so the container itself
must reach the endpoint over HTTPS. `--allow-agent-host` adds the endpoint to
the agent's allowlist only for tasks with a restricted network policy; Harbor
ignores it for public tasks such as TBLite. Daytona Tier 1 and Tier 2 sandboxes
can reach only Daytona's essential-services list whatever the task allows, so a
self-hosted endpoint needs Harbor's local `docker` environment, Daytona Tier 3
or higher, or another provider with open outbound access.

```sh
export S_CODE_HARBOR_CREDENTIAL='<credential>'
export S_CODE_HARBOR_BUNDLE='.work/harbor/s-code-<commit>-x86_64-unknown-linux-gnu.tar.gz'
export S_CODE_HARBOR_BUNDLE_SHA256='<digest from the .sha256 file>'
export PYTHONPATH="$PWD${PYTHONPATH:+:$PYTHONPATH}"

harbor run \
  --dataset '<dataset@version>' \
  --agent integrations.harbor.agent:SCodeAgent \
  --model '<endpoint-model-id>' \
  --agent-kwarg "bundle_path=${S_CODE_HARBOR_BUNDLE}" \
  --agent-kwarg "bundle_sha256=${S_CODE_HARBOR_BUNDLE_SHA256}" \
  --agent-kwarg 'model_base_url=https://models.example/v1' \
  --agent-kwarg 'credential_env=S_CODE_HARBOR_CREDENTIAL' \
  --agent-env "S_CODE_HARBOR_CREDENTIAL=${S_CODE_HARBOR_CREDENTIAL}" \
  --allow-agent-host models.example \
  --jobs-dir .work/harbor-jobs
```

`bundle_path`, `bundle_sha256`, `model_base_url`, `credential_env`, and the
Harbor `--model` value are required. The credential variable must have a
nonempty value, including when the endpoint does not authenticate, and a name
that starts with `S_CODE_HARBOR_`. The adapter passes the model name through to
S-Code and records only the credential handle, not its value. Optional agent
kwargs:

| Kwarg | Default | Meaning |
| --- | --- | --- |
| `max_model_calls` | 100 | Model calls allowed in the turn |
| `max_tool_calls` | 256 | Tool calls allowed in the turn |
| `model_stream_idle_seconds` | 300 | Longest gap between model stream events |
| `model_connect_timeout_sec` | 10 | Time allowed to open one model connection |
| `model_read_timeout_sec` | 600 | Longest gap between bytes of one model response |
| `model_request_timeout_sec` | 3600 | Time allowed for one model request |
| `temperature` | S-Code's default, 0 | Sampling temperature, 0 to 2 |
| `max_output_tokens` | S-Code's default, 8192 growing to 32768 | Fixed output-token cap per model call; a reasoning model's hidden reasoning counts toward it |

Set the sampling settings with these kwargs; the adapter refuses
`S_CODE_MODEL_TEMPERATURE` and `S_CODE_MODEL_MAX_OUTPUT_TOKENS` in the agent
env, so provenance always records the values used.

The adapter reads Harbor's resolved per-task agent deadline and nests the model
request, S-Code turn, CLI, cleanup, and Harbor limits inside it. Requested model
timeouts are automatically clamped for shorter tasks.

The adapter validates the archive and its manifest locally, uploads that exact
archive, checks its SHA-256 inside the task container, extracts it, runs each
binary's self-test, and uses fresh S-Code state for the trial. The adapter
selects the additive `--raw-events-jsonl` output mode; existing `--jsonl` and
`--stream-json` output contracts are unchanged. It preserves these files in
Harbor's agent logs:

- `events.jsonl`: every raw S-Code `LiveEvent`, as versioned JSON Lines.
- `stderr.log`: S-Code diagnostic output.
- `final.txt`: the final agent message.
- `provenance.json`: adapter, bundle, model, endpoint, and trial metadata.
  `adapter.source_sha256` is what
  `LC_ALL=C sh -c 'sha256sum __init__.py agent.py runtime.py | sha256sum'`
  prints in this directory, and `execution.unmanaged_s_code_env` names any
  other `S_CODE_*` variables passed with `--agent-env`.
- `daemon.log`: the S-Code daemon's log, including why model requests failed.
- `command-env.json`: the names of the variables S-Code's commands receive and
  a SHA-256 of them, never the values. The digest covers the `NAME=VALUE`
  records in byte order of their names, joined by single NUL bytes with no
  trailing NUL. Commands also receive `PWD`, which is not listed.

If no model request in the run gets a successful response, provenance records
`model_unreachable` or `model_rejected`. The adapter raises Harbor's
`NetworkConnectionError` for transport failures and `UnknownApiError` for HTTP
rejections, allowing Harbor to record the infrastructure failure and still run
the verifier.

S-Code runs in Full permission mode because the Harbor task container is the
outer filesystem and network boundary. Browser, PDF, and interactive-input tools
are disabled because they cannot complete reliably in an unattended trial. The
adapter also has S-Code send its system messages as one leading message,
because some chat templates, including Qwen3.6's, reject later ones. The
adapter performs one attempt and does not provide retries, resume, persistent
memory, or ATIF conversion.

## Command environment

Task images often install tools under `/usr/local/bin`, add directories with
`ENV PATH`, or set `HOME`, `JAVA_HOME` or `PYTHONPATH`. S-Code normally runs
Full-mode commands with a cleared environment and a minimal `PATH`, so the
Harbor bundle has a container environment mode instead:

- Before it sets anything of its own, the runner saves its environment as the
  agent user: the environment an ordinary Harbor command gets in the task
  container, minus every `--agent-env` variable. The daemon loads that file
  once at startup and deletes it.
- Full-mode commands get that environment, with `PWD` set to their working
  directory, and programs are found on its `PATH`. S-Code adds no `PATH`,
  `TMPDIR` or `OPENSSL_CONF` of its own.
- Commands never receive `--agent-env` variables (the credential among them),
  `S_CODE_*` variables, any variable whose value is the model credential, or
  the shell's `PWD`, `OLDPWD`, `SHLVL` and `_`. A task variable that shares its
  name with an `--agent-env` variable is dropped as well. The daemon loads the
  snapshot only if it is a regular file that no other user can read.
- `credential_env` must start with `S_CODE_HARBOR_`, so the credential is also
  excluded by its prefix and cannot collide with S-Code's own settings.
- Only this bundle can enable the mode. `build-bundle.sh` builds the daemon
  with the `harbor-container-env` Cargo feature, which other builds leave off,
  and the adapter refuses a bundle without it. Sandboxed commands, extensions
  and other S-Code builds are unchanged.

Provenance copies the names and digest from `command-env.json` into
`execution.command_environment`, and sets `execution.host_command_env` to
`container` once that summary is read, or to `unverified` with the reason in
`execution.command_environment_error`.

## Audit a run

Before using a run's scores, check that every trial measured S-Code rather
than the harness:

```sh
python3 integrations/harbor/audit.py .work/harbor-jobs/<job-name>
```

The audit flags a trial when Harbor did not finish it, its task image failed to
build, S-Code never got a model response, the trial was cancelled, S-Code's
cleanup failed, provenance does not show `host_command_env` as `container`, or
the verifier reported less than full credit without a test summary, which
usually means its tests never ran. The summary check applies to
tasks whose `tests/test.sh` runs pytest, as all of Terminal-Bench 2 and 97 of
the 100 TBLite tasks do. Other verifiers print no common summary, so the audit
keeps their reward and adds a warning, unless their output is empty or shows a
known setup failure. The audit reads the test script from the task folder
recorded in the trial's `config.json`; if that folder isn't present where the
audit runs, it flags the trial as before. Rerun flagged trials instead of
scoring them. The command exits with status 1 when it flags any trial, and
`--json` prints machine-readable results.
