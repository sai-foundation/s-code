#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"
mkdir -p "$ROOT/.work"
task="$(mktemp -d "$ROOT/.work/first-run.XXXXXX")"
daemon_pid=""
model_pid=""
unrelated_pid=""
stage="initialization"
cleanup() {
  status=$?
  trap - 0 1 2 15
  if [ -n "$daemon_pid" ]; then
    kill "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
  fi
  if [ -n "$unrelated_pid" ]; then
    kill "$unrelated_pid" 2>/dev/null || true
    wait "$unrelated_pid" 2>/dev/null || true
  fi
  if [ -n "$model_pid" ]; then
    kill "$model_pid" 2>/dev/null || true
    wait "$model_pid" 2>/dev/null || true
  fi
  if [ "$status" -eq 0 ]; then
    find "$task" -depth -delete
  else
    echo "first-run test failed during $stage; retained task artifacts at $task" >&2
  fi
  exit "$status"
}
trap cleanup 0 1 2 15

private_mode() {
  if stat -f '%Lp' "$1" >/dev/null 2>&1; then
    stat -f '%Lp' "$1"
  else
    stat -c '%a' "$1"
  fi
}

for command in git openssl python3; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "$command is required" >&2
    exit 1
  }
done

mkdir -p "$task/bin" "$task/home" "$task/tmp" "$task/workspace"
if [ -n "${OPENCODING_SHARED_TEST_TMPDIR:-}" ]; then
  case "$OPENCODING_SHARED_TEST_TMPDIR" in
    "$ROOT/.work/"*) ;;
    *) echo "shared test TMPDIR must be below $ROOT/.work" >&2; exit 2 ;;
  esac
  mkdir -p "$OPENCODING_SHARED_TEST_TMPDIR"
  export TMPDIR="$OPENCODING_SHARED_TEST_TMPDIR"
else
  export TMPDIR="$task/tmp"
fi
chmod 0700 "$TMPDIR"
build_target="${CARGO_TARGET_DIR:-$task/target}"
stage="building the installed CLI and local service"
if [ "${OPENCODING_SKIP_BUILD:-0}" != "1" ] && [ -z "${OPENCODING_TEST_BIN_DIR:-}" ]; then
  CARGO_TARGET_DIR="$build_target" cargo build --locked \
    -p opencoding-daemon -p opencoding-cli >/dev/null
fi
binary_dir="${OPENCODING_TEST_BIN_DIR:-$build_target/debug}"
command_bin="${OPENCODING_TEST_BIN_DIR:-$task/bin}"
for binary in opencoding-daemon opencoding-cli; do
  [ -x "$binary_dir/$binary" ] || {
    echo "missing built binary: $binary_dir/$binary" >&2
    exit 1
  }
  if [ -z "${OPENCODING_TEST_BIN_DIR:-}" ]; then
    install -m 0755 "$binary_dir/$binary" "$command_bin/$binary"
  fi
done
if [ -z "${OPENCODING_TEST_BIN_DIR:-}" ]; then
  install -m 0755 "$ROOT/scripts/opencoding" "$command_bin/opencoding"
fi
[ -x "$command_bin/opencoding" ] || { echo "missing installed launcher" >&2; exit 1; }

printf 'original\n' >"$task/workspace/tracked.txt"
git -C "$task/workspace" init -q
git -C "$task/workspace" -c user.name='First Run' -c user.email='first-run@example.invalid' \
  add tracked.txt
git -C "$task/workspace" -c user.name='First Run' -c user.email='first-run@example.invalid' \
  commit -q -m initial
expected_sha256="$(openssl dgst -sha256 "$task/workspace/tracked.txt" | awk '{print $NF}')"

env FIXTURE_EXPECTED_SHA256="$expected_sha256" \
  python3 "$ROOT/tests/model_fixture.py" "$task/model.addr" \
  >"$task/model.out" 2>"$task/model.err" &
model_pid=$!
attempt=0
while [ ! -s "$task/model.addr" ] && [ "$attempt" -lt 1200 ]; do
  kill -0 "$model_pid" 2>/dev/null || { sed -n '1,120p' "$task/model.err" >&2; exit 1; }
  attempt=$((attempt + 1))
  sleep 0.05
done
[ -s "$task/model.addr" ] || { echo "fixture model did not become ready" >&2; exit 1; }
model_address="$(sed -n '1p' "$task/model.addr")"

export HOME="$task/home"
export OPENCODING_RUNTIME_DIR="$task/runtime"
export OPENCODING_STATE_DIR="$task/state"
export OPENCODING_ORGANIZATION="org-first-run"
export OPENCODING_TEAM="team-first-run"
export OPENCODING_ACTOR="user-first-run"
export OPENCODING_WORKSPACE="file://$task/workspace"
export FIRST_RUN_MODEL_KEY="fixture-secret"

# OPENCODING_HOME is the shared contract for setup, autostart, doctor and Web.
# Keep HOME elsewhere so the test catches clients that silently fall back to it.
export OPENCODING_HOME="$task/product-home"

# Launcher bookkeeping and the browser opener must never resolve through an
# untrusted workspace/caller PATH. Those utilities parse the private daemon
# connection or receive a single-use browser bootstrap URL.
untrusted_bin="$task/untrusted-bin"
mkdir -p "$untrusted_bin"
for utility in dirname sed head wc nohup open xdg-open; do
  marker="$task/untrusted-$utility-ran"
  printf '#!/bin/sh\n: > %s\nexit 97\n' "$(printf '%s' "$marker" | sed "s/'/'\\''/g")" >"$untrusted_bin/$utility"
  chmod 0700 "$untrusted_bin/$utility"
done

started_at="$(date +%s)"
# Reconfiguration must replace a daemon that already loaded an obsolete model
# endpoint. Otherwise doctor can report the old process as healthy while the
# first real task still uses stale settings.
stage="starting the deliberately unavailable model configuration"
PATH="$untrusted_bin:$PATH" "$command_bin/opencoding" setup \
  --provider openai-compatible \
  --base-url "http://$model_address/unavailable" \
  --credential-handle FIRST_RUN_MODEL_KEY \
  --model fixture/stale \
  --yes >"$task/setup-stale.out"
PATH="$untrusted_bin:$PATH" OPENCODING_NO_BROWSER=1 "$command_bin/opencoding" web >"$task/web-stale.out"
for utility in dirname sed head wc nohup open xdg-open; do
  [ ! -e "$task/untrusted-$utility-ran" ] || {
    echo "launcher executed untrusted $utility from caller PATH" >&2
    exit 1
  }
done
connection="$task/runtime/daemon.json"
stale_daemon_pid="$(sed -n 's/.*"pid":[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$connection" | head -n 1)"
[ -n "$stale_daemon_pid" ] && kill -0 "$stale_daemon_pid" 2>/dev/null
if "$command_bin/opencoding" doctor >"$task/doctor-stale.out" 2>"$task/doctor-stale.err"; then
  echo "doctor accepted an unreachable model endpoint" >&2
  exit 1
fi
grep -F 'model endpoint readiness failed' "$task/doctor-stale.out" >/dev/null

stage="writing the updated model configuration"
"$command_bin/opencoding" setup \
  --provider openai-compatible \
  --base-url "http://$model_address" \
  --credential-handle FIRST_RUN_MODEL_KEY \
  --model fixture/model \
  --yes >"$task/setup.out"
stage="verifying the local service replacement"
reconfigured_daemon_pid="$(sed -n 's/.*"pid":[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$connection" | head -n 1)"
[ -n "$reconfigured_daemon_pid" ] || {
  echo "setup did not publish a replacement local service" >&2
  exit 1
}
[ "$reconfigured_daemon_pid" != "$stale_daemon_pid" ] || {
  echo "setup did not replace the service that loaded the stale configuration" >&2
  exit 1
}
if kill -0 "$stale_daemon_pid" 2>/dev/null; then
  echo "setup left the stale local service running" >&2
  exit 1
fi
grep -F 'local service restarted with the updated configuration' "$task/setup.out" >/dev/null
config="$task/product-home/config.toml"
[ -f "$config" ] && [ ! -L "$config" ]
if grep -F 'fixture-secret' "$config" >/dev/null; then
  echo "setup persisted the provider secret" >&2
  exit 1
fi

stage="checking concurrent health diagnostics"
"$command_bin/opencoding" doctor >"$task/doctor-one.out" &
doctor_one_pid=$!
"$command_bin/opencoding" doctor >"$task/doctor-two.out" &
doctor_two_pid=$!
wait "$doctor_one_pid"
wait "$doctor_two_pid"
for doctor_output in "$task/doctor-one.out" "$task/doctor-two.out"; do
  grep -F 'daemon healthy' "$doctor_output" >/dev/null
  grep -F 'storage encrypted with a private managed key' "$doctor_output" >/dev/null
  grep -F 'model endpoint catalog reachable; credential handle present' "$doctor_output" >/dev/null
  if grep -F 'credential accepted' "$doctor_output" >/dev/null; then
    echo "doctor claimed that a catalog request proved credential acceptance" >&2
    exit 1
  fi
done
stage="checking private runtime state and restart evidence"
[ "$(private_mode "$task/runtime")" = "700" ]
[ "$(private_mode "$task/state")" = "700" ]
daemon_start_count="$(grep -c 'OPENCODING_ADDR=' "$task/state/logs/daemon.log")"
[ "$daemon_start_count" -eq 2 ] || {
  echo "expected two local service starts after reconfiguration; found $daemon_start_count" >&2
  exit 1
}

# A slow or wedged daemon may still own live turns and terminals. Automatic
# startup must return within a small deadline without killing it; only the
# explicit restart command may replace the recorded instance.
stage="recovering explicitly from an unresponsive local service"
"$command_bin/opencoding" stop >/dev/null
wedged_address_file="$task/wedged.addr"
python3 "$ROOT/tests/wedged-opencoding-daemon.py" "$wedged_address_file" "$task/runtime/daemon.lock" \
  >"$task/wedged.out" 2>"$task/wedged.err" &
daemon_pid=$!
attempt=0
while [ ! -s "$wedged_address_file" ] && [ "$attempt" -lt 100 ]; do
  kill -0 "$daemon_pid" 2>/dev/null || { cat "$task/wedged.err" >&2; exit 1; }
  attempt=$((attempt + 1))
  sleep 0.05
done
wedged_address="$(sed -n '1p' "$wedged_address_file")"
umask 077
printf '%s\n' "{\"schema_version\":2,\"daemon_url\":\"http://$wedged_address\",\"token\":\"wedged-token\",\"instance_id\":\"wedged-instance-0001\",\"pid\":$daemon_pid,\"started_at\":\"2026-01-01T00:00:00Z\"}" >"$connection"
recovery_started="$(date +%s)"
if "$command_bin/opencoding" doctor >"$task/doctor-after-wedge.out" 2>"$task/doctor-after-wedge.err"; then
  echo "automatic startup replaced an unresponsive live daemon" >&2
  exit 1
fi
recovery_elapsed=$(( $(date +%s) - recovery_started ))
[ "$recovery_elapsed" -lt 8 ] || {
  echo "unresponsive daemon detection exceeded eight seconds: ${recovery_elapsed}s" >&2
  exit 1
}
kill -0 "$daemon_pid" 2>/dev/null || {
  echo "automatic startup killed an unresponsive live daemon" >&2
  exit 1
}
grep -F 'running but did not answer its health check' "$task/doctor-after-wedge.err" >/dev/null
"$command_bin/opencoding" restart >"$task/restart-after-wedge.out"
if kill -0 "$daemon_pid" 2>/dev/null; then
  echo "explicit restart did not stop the wedged daemon" >&2
  exit 1
fi
daemon_pid=""
"$command_bin/opencoding" doctor >"$task/doctor-after-wedge.out"
grep -F 'daemon healthy' "$task/doctor-after-wedge.out" >/dev/null

first_daemon_pid="$(sed -n 's/.*"pid":[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$connection" | head -n 1)"
web_launch="$(OPENCODING_NO_BROWSER=1 "$command_bin/opencoding" web)"
case "$web_launch" in
  'Local Web (single-use; do not share): http://127.0.0.1:'*'#opencoding-bootstrap='*) ;;
  *) echo "headless Local Web did not return an authenticated single-use URL" >&2; exit 1 ;;
esac
unset web_launch
reused_daemon_pid="$(sed -n 's/.*"pid":[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$connection" | head -n 1)"
[ "$reused_daemon_pid" = "$first_daemon_pid" ]
[ "$(grep -c 'OPENCODING_ADDR=' "$task/state/logs/daemon.log")" -eq 3 ]

stage="running the first real coding task"
"$command_bin/opencoding" exec --permission-mode accept-edits --timeout 30 \
  "Replace tracked.txt with the approved first-run content." >"$task/exec.out"
grep -Fx 'write complete' "$task/exec.out" >/dev/null
grep -Fx 'approved via cli' "$task/workspace/tracked.txt" >/dev/null

[ -f "$connection" ] && [ ! -L "$connection" ]
daemon_pid="$(sed -n 's/.*"pid":[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$connection" | head -n 1)"
[ -n "$daemon_pid" ] && kill -0 "$daemon_pid" 2>/dev/null
[ -f "$task/state/opencoding.db" ]
[ -f "$task/state/.opencoding.db.storage-key" ]

# An unclean daemon exit leaves the advisory lock file behind, but the OS lock
# itself is released. The next public command must recover without a delay or
# a manual repair step.
stage="recovering after an unclean local-service exit"
kill -9 "$daemon_pid"
wait "$daemon_pid" 2>/dev/null || true
daemon_pid=""
mkdir -p "$task/runtime/autostart.lock"
"$command_bin/opencoding" doctor >"$task/doctor-after-crash.out"
grep -F 'daemon healthy' "$task/doctor-after-crash.out" >/dev/null
daemon_pid="$(sed -n 's/.*"pid":[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$connection" | head -n 1)"
[ -n "$daemon_pid" ] && [ "$daemon_pid" != "$first_daemon_pid" ]
[ -f "$task/runtime/daemon.lock" ]

# A stale private connection file can point at a PID that the operating system
# has reused. Recovery may replace that file only when the daemon instance lock
# is free; it must never signal the unrelated live process.
stage="recovering safely from a reused process id"
"$command_bin/opencoding" stop >/dev/null
daemon_pid=""
sleep 60 &
unrelated_pid=$!
printf '%s\n' "{\"schema_version\":2,\"daemon_url\":\"http://127.0.0.1:9\",\"token\":\"stale-token\",\"instance_id\":\"stale-instance-0001\",\"pid\":$unrelated_pid,\"started_at\":\"2026-01-01T00:00:00Z\"}" >"$connection"
"$command_bin/opencoding" doctor >"$task/doctor-after-pid-reuse.out"
kill -0 "$unrelated_pid" 2>/dev/null || {
  echo "stale connection recovery killed an unrelated process" >&2
  exit 1
}
kill "$unrelated_pid"
wait "$unrelated_pid" 2>/dev/null || true
unrelated_pid=""
grep -F 'daemon healthy' "$task/doctor-after-pid-reuse.out" >/dev/null
daemon_pid="$(sed -n 's/.*"pid":[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$connection" | head -n 1)"

# Follow the documented doctor -> stop -> offline maintenance sequence. Pure
# informational/config commands must not recover or mutate a restore journal.
stage="running offline maintenance"
"$command_bin/opencoding" stop >"$task/stop.out"
daemon_pid=""
[ ! -e "$connection" ]
restore_marker="$task/state/.opencoding.db.restore-pending.json"
printf '%s\n' 'restore-journal-must-remain-byte-identical' >"$restore_marker"
marker_sha256="$(openssl dgst -sha256 "$restore_marker" | awk '{print $NF}')"
"$command_bin/opencoding" web --help >"$task/help.out"
"$command_bin/opencoding" web --version >"$task/version.out"
"$command_bin/opencoding" web --self-test >"$task/self-test.out"
"$command_bin/opencoding" web --config-validate >"$task/config-validate.out"
"$command_bin/opencoding" web --config-print-effective >"$task/config-effective.out"
[ "$(openssl dgst -sha256 "$restore_marker" | awk '{print $NF}')" = "$marker_sha256" ]
[ ! -e "$connection" ]
rm "$restore_marker"

backup="$task/backup/opencoding-backup.sqlite"
mkdir -p "$task/backup"
"$command_bin/opencoding" web --backup "$backup" >"$task/backup.out"
"$command_bin/opencoding" web --verify-database >"$task/verify-database.out"
[ -f "$backup" ]
[ -f "$task/backup/.opencoding-backup.sqlite.storage-key" ]
grep -F 'opencoding database integrity ok' "$task/verify-database.out" >/dev/null

elapsed=$(( $(date +%s) - started_at ))
[ "$elapsed" -lt 600 ] || {
  echo "first usable task exceeded ten minutes: ${elapsed}s" >&2
  exit 1
}
echo "setup, secure autostart, doctor, first real task, and offline maintenance passed in ${elapsed}s"
