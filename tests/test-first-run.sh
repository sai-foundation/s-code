#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"
mkdir -p "$ROOT/.work"
task="$(mktemp -d "$ROOT/.work/first-run.XXXXXX")"
daemon_pid=""
model_pid=""
cleanup() {
  status=$?
  trap - 0 1 2 15
  if [ -n "$daemon_pid" ]; then
    kill "$daemon_pid" 2>/dev/null || true
    wait "$daemon_pid" 2>/dev/null || true
  fi
  if [ -n "$model_pid" ]; then
    kill "$model_pid" 2>/dev/null || true
    wait "$model_pid" 2>/dev/null || true
  fi
  if [ "$status" -eq 0 ]; then
    find "$task" -depth -delete
  else
    echo "first-run test failed; retained task artifacts at $task" >&2
  fi
  exit "$status"
}
trap cleanup 0 1 2 15

for command in git openssl python3; do
  command -v "$command" >/dev/null 2>&1 || {
    echo "$command is required" >&2
    exit 1
  }
done

mkdir -p "$task/bin" "$task/home" "$task/runtime" "$task/state" "$task/tmp" "$task/workspace"
export TMPDIR="$task/tmp"
build_target="${CARGO_TARGET_DIR:-$task/target}"
if [ "${OPENCODING_SKIP_BUILD:-0}" != "1" ]; then
  CARGO_TARGET_DIR="$build_target" cargo build --locked \
    -p opencoding-daemon -p opencoding-cli >/dev/null
fi
for binary in opencoding-daemon opencoding-cli; do
  [ -x "$build_target/debug/$binary" ] || {
    echo "missing built binary: $build_target/debug/$binary" >&2
    exit 1
  }
  install -m 0755 "$build_target/debug/$binary" "$task/bin/$binary"
done
install -m 0755 "$ROOT/scripts/opencoding" "$task/bin/opencoding"

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
while [ ! -s "$task/model.addr" ] && [ "$attempt" -lt 100 ]; do
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

started_at="$(date +%s)"
"$task/bin/opencoding" setup \
  --provider openai-compatible \
  --base-url "http://$model_address" \
  --credential-handle FIRST_RUN_MODEL_KEY \
  --model fixture/model \
  --yes >"$task/setup.out"
config="$task/home/.opencoding/config.toml"
[ -f "$config" ] && [ ! -L "$config" ]
if grep -F 'fixture-secret' "$config" >/dev/null; then
  echo "setup persisted the provider secret" >&2
  exit 1
fi

"$task/bin/opencoding" doctor >"$task/doctor-one.out" &
doctor_one_pid=$!
"$task/bin/opencoding" doctor >"$task/doctor-two.out" &
doctor_two_pid=$!
wait "$doctor_one_pid"
wait "$doctor_two_pid"
for doctor_output in "$task/doctor-one.out" "$task/doctor-two.out"; do
  grep -F 'daemon healthy' "$doctor_output" >/dev/null
  grep -F 'storage encrypted with a private managed key' "$doctor_output" >/dev/null
  grep -F 'model endpoint and credential handle configured' "$doctor_output" >/dev/null
done
[ "$(grep -c 'OPENCODING_ADDR=' "$task/state/logs/daemon.log")" -eq 1 ]

"$task/bin/opencoding" exec --permission-mode accept-edits --timeout 30 \
  "Replace tracked.txt with the approved first-run content." >"$task/exec.out"
grep -Fx 'write complete' "$task/exec.out" >/dev/null
grep -Fx 'approved via cli' "$task/workspace/tracked.txt" >/dev/null

connection="$task/runtime/daemon.json"
[ -f "$connection" ] && [ ! -L "$connection" ]
daemon_pid="$(sed -n 's/.*"pid":[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$connection" | head -n 1)"
[ -n "$daemon_pid" ] && kill -0 "$daemon_pid" 2>/dev/null
[ -f "$task/state/opencoding.db" ]
[ -f "$task/state/.opencoding.db.storage-key" ]

elapsed=$(( $(date +%s) - started_at ))
[ "$elapsed" -lt 600 ] || {
  echo "first usable task exceeded ten minutes: ${elapsed}s" >&2
  exit 1
}
echo "setup, secure autostart, doctor, and first real task passed in ${elapsed}s"
