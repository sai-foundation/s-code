#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"
mkdir -p "$ROOT/.work"
tmp="$(mktemp -d "$ROOT/.work/cli-e2e.XXXXXX")"
daemon_pid=""
model_pid=""
api_server_pid=""
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
  if [ -n "$api_server_pid" ]; then
    kill "$api_server_pid" 2>/dev/null || true
    wait "$api_server_pid" 2>/dev/null || true
  fi
  if [ "$status" -eq 0 ]; then
    find "$tmp" -depth -delete
  else
    echo "CLI E2E failed; retained task artifacts at $tmp" >&2
  fi
  exit "$status"
}
trap cleanup 0 1 2 15

command -v curl >/dev/null 2>&1 || { echo "curl is required" >&2; exit 1; }
command -v ruby >/dev/null 2>&1 || { echo "ruby is required" >&2; exit 1; }
command -v python3 >/dev/null 2>&1 || { echo "python3 is required" >&2; exit 1; }
mkdir -p "$tmp/tmp"
if [ -n "${OPENCODING_SHARED_TEST_TMPDIR:-}" ]; then
  case "$OPENCODING_SHARED_TEST_TMPDIR" in
    "$ROOT/.work/"*) ;;
    *) echo "shared test TMPDIR must be below $ROOT/.work" >&2; exit 2 ;;
  esac
  mkdir -p "$OPENCODING_SHARED_TEST_TMPDIR"
  export TMPDIR="$OPENCODING_SHARED_TEST_TMPDIR"
else
  export TMPDIR="$tmp/tmp"
fi
chmod 0700 "$TMPDIR"

if [ "${OPENCODING_SKIP_BUILD:-0}" != "1" ]; then
  build_target="${CARGO_TARGET_DIR:-$tmp/target}"
  export CARGO_TARGET_DIR="$build_target"
  cargo build --locked -p opencoding-daemon -p opencoding-cli \
    -p opencoding-api-server >/dev/null
else
  [ -n "${CARGO_TARGET_DIR:-}" ] || {
    echo "CARGO_TARGET_DIR is required when OPENCODING_SKIP_BUILD=1" >&2
    exit 2
  }
  build_target="$CARGO_TARGET_DIR"
fi
daemon="$build_target/debug/opencoding-daemon"
cli="$build_target/debug/opencoding-cli"
api_server="$build_target/debug/opencoding-api-server"
[ -x "$daemon" ] && [ -x "$cli" ] && [ -x "$api_server" ]

if "$cli" exec --timeout 0 "invalid timeout" \
  >"$tmp/invalid-usage.out" 2>"$tmp/invalid-usage.err"; then
  echo "invalid CLI usage unexpectedly succeeded" >&2
  exit 1
else
  exit_status=$?
fi
[ "$exit_status" -eq 2 ] || {
  echo "invalid CLI usage returned $exit_status instead of 2" >&2
  cat "$tmp/invalid-usage.err" >&2
  exit 1
}
[ ! -s "$tmp/invalid-usage.out" ]
grep -F 'invalid CLI usage: --timeout must be between 1 and 86400 seconds' \
  "$tmp/invalid-usage.err" >/dev/null

token="cli-e2e-token"
workspace="$tmp/workspace"
mkdir -p "$workspace"
printf 'original\n' > "$workspace/tracked.txt"
git -C "$workspace" init -q
git -C "$workspace" -c user.name='CLI E2E' -c user.email='cli@example.invalid' \
  add tracked.txt
git -C "$workspace" -c user.name='CLI E2E' -c user.email='cli@example.invalid' \
  commit -q -m initial
expected_sha256="$(openssl dgst -sha256 "$workspace/tracked.txt" | awk '{print $NF}')"
env FIXTURE_EXPECTED_SHA256="$expected_sha256" \
  python3 "$ROOT/tests/model_fixture.py" "$tmp/model.addr" \
  >"$tmp/model.out" 2>"$tmp/model.err" &
model_pid=$!
attempt=0
while [ ! -s "$tmp/model.addr" ] && [ "$attempt" -lt 300 ]; do
  kill -0 "$model_pid" 2>/dev/null || { cat "$tmp/model.err" >&2; exit 1; }
  attempt=$((attempt + 1))
  sleep 0.1
done
[ -s "$tmp/model.addr" ] || {
  cat "$tmp/model.err" >&2
  echo "fixture model did not become ready" >&2
  exit 1
}
model_address="$(sed -n '1p' "$tmp/model.addr")"
[ -n "$model_address" ] || { echo "fixture model did not become ready" >&2; exit 1; }

env OPENCODING_API_SERVER_BIND="127.0.0.1:0" \
  OPENCODING_API_SERVER_UPSTREAM="http://$model_address" \
  OPENROUTER_API_KEY="fixture-secret" \
  "$api_server" >"$tmp/api-server.out" 2>"$tmp/api-server.err" &
api_server_pid=$!
api_server_address=""
attempt=0
while [ "$attempt" -lt 300 ]; do
  api_server_address="$(sed -n 's/^OPENCODING_API_SERVER_ADDR=//p' "$tmp/api-server.err" | tail -n 1)"
  [ -z "$api_server_address" ] || break
  kill -0 "$api_server_pid" 2>/dev/null || { cat "$tmp/api-server.err" >&2; exit 1; }
  attempt=$((attempt + 1))
  sleep 0.1
done
[ -n "$api_server_address" ] || { echo "API Server did not become ready" >&2; exit 1; }
export OPENCODING_MODEL_BASE_URL="http://$api_server_address/v1"

env OPENCODING_RUNTIME_DIR="$tmp/run" \
  OPENCODING_DATABASE_URL="sqlite://$tmp/state.db" OPENCODING_TOKEN="$token" \
  OPENCODING_MODEL_BASE_URL="http://$api_server_address/v1" \
  "$daemon" >"$tmp/daemon.out" 2>"$tmp/daemon.err" &
daemon_pid=$!

address=""
attempt=0
while [ "$attempt" -lt 300 ]; do
  address="$(sed -n 's/^OPENCODING_ADDR=//p' "$tmp/daemon.err" | tail -n 1)"
  [ -z "$address" ] || break
  kill -0 "$daemon_pid" 2>/dev/null || { cat "$tmp/daemon.err" >&2; exit 1; }
  attempt=$((attempt + 1))
  sleep 0.1
done
[ -n "$address" ] || { echo "daemon did not become ready" >&2; exit 1; }
attempt=0
while [ ! -s "$tmp/run/daemon.json" ] && [ "$attempt" -lt 300 ]; do
  kill -0 "$daemon_pid" 2>/dev/null || { cat "$tmp/daemon.err" >&2; exit 1; }
  attempt=$((attempt + 1))
  sleep 0.1
done
[ -s "$tmp/run/daemon.json" ] || {
  echo "daemon connection file did not become ready" >&2
  exit 1
}
url="http://$address"

export OPENCODING_RUNTIME_DIR="$tmp/run"
export OPENCODING_ORGANIZATION="org-e2e"
export OPENCODING_TEAM="team-e2e"
export OPENCODING_ACTOR="user-e2e"
export OPENCODING_WORKSPACE="file://$workspace"
export OPENCODING_MODEL="gpt-5"

# A bare loopback request cannot mint Local Web authority. Only the launcher
# holding the private daemon connection can create a one-time fragment, and
# that fragment exchanges exactly once for an HttpOnly cookie.
curl --fail --silent --show-error "$url/" | ruby -e '
  html = STDIN.read
  abort "bare Local Web page exposed a bootstrap" unless html.include?(%q{meta name="opencoding-bootstrap" content=""})
'
unauthenticated_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
  --request POST -H 'x-opencoding-csrf: 1' "$url/v1/auth/browser-bootstrap")"
[ "$unauthenticated_status" = "401" ] || {
  echo "unauthenticated local process minted a browser bootstrap" >&2
  exit 1
}
launch_url="$("$cli" --local-web-launch-url)"
case "$launch_url" in
  "$url/#opencoding-bootstrap="*) ;;
  *) echo "authenticated launcher returned an invalid Local Web URL" >&2; exit 1 ;;
esac
browser_bootstrap="${launch_url##*#opencoding-bootstrap=}"
exchange_headers="$(curl --silent --show-error --include --request POST \
  -H 'Content-Type: application/json' -H 'x-opencoding-csrf: 1' \
  --data "{\"token\":\"$browser_bootstrap\"}" "$url/v1/auth/bootstrap")"
browser_cookie="$(printf '%s\n' "$exchange_headers" | sed -n 's/^[Ss]et-[Cc]ookie: \([^;]*\).*/\1/p' | tr -d '\r')"
[ -n "$browser_cookie" ] || { echo "browser bootstrap did not yield a cookie" >&2; exit 1; }
browser_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
  -H "Cookie: $browser_cookie" "$url/v1/capabilities")"
[ "$browser_status" = "200" ] || { echo "launcher-created browser cookie was rejected" >&2; exit 1; }
replay_status="$(curl --silent --output /dev/null --write-out '%{http_code}' \
  --request POST -H 'Content-Type: application/json' -H 'x-opencoding-csrf: 1' \
  --data "{\"token\":\"$browser_bootstrap\"}" "$url/v1/auth/bootstrap")"
[ "$replay_status" = "401" ] || { echo "browser bootstrap was replayable" >&2; exit 1; }

# The PTY driver waits for each rendered state before sending the next key.
python3 "$ROOT/tests/cli_pty_driver.py" "$cli" "$tmp/create.transcript" \
  create "file://$workspace"
curl --fail --silent --show-error \
  -H "Authorization: Bearer $token" \
  "$url/v1/sessions?organization_id=org-e2e&team_id=team-e2e&actor_id=user-e2e" \
  > "$tmp/sessions.json"
ruby -rjson -e '
  sessions = JSON.parse(File.read(ARGV.fetch(0)))
  abort "CLI did not create exactly one session" unless sessions.length == 1
  abort "wrong Team" unless sessions[0]["scope"]["team_id"] == "team-e2e"
  abort "wrong workspace" unless sessions[0]["workspace_uri"] == ARGV.fetch(1)
  abort "wrong model" unless sessions[0]["model"] == "gpt-5"
' "$tmp/sessions.json" "file://$workspace"

# Exercise the exact session endpoint used by web/app.js, then prove a fresh
# CLI process renders that Web-created session from the shared daemon store.
curl --fail --silent --show-error \
  -H "Authorization: Bearer $token" \
  -H 'Content-Type: application/json' \
  -H 'x-opencoding-csrf: 1' \
  -d "{\"scope\":{\"organization_id\":\"org-e2e\",\"team_id\":\"team-e2e\",\"actor_id\":\"user-e2e\",\"goal_id\":null,\"task_id\":null},\"workspace_uri\":\"file://$workspace\",\"title\":\"Web Shared Session\",\"model\":\"gpt-5\"}" \
  "$url/v1/sessions" > "$tmp/web-session.json"
python3 "$ROOT/tests/cli_pty_driver.py" "$cli" "$tmp/restore.transcript" \
  restore "Web"
grep -a 'Web' "$tmp/restore.transcript" >/dev/null
grep -a 'Session' "$tmp/restore.transcript" >/dev/null
python3 "$ROOT/tests/cli_pty_driver.py" "$cli" "$tmp/picker.transcript" picker
grep -a 'Model picker' "$tmp/picker.transcript" >/dev/null
grep -a 'Permission picker' "$tmp/picker.transcript" >/dev/null
grep -a 'Accept edits' "$tmp/picker.transcript" >/dev/null
python3 "$ROOT/tests/cli_pty_driver.py" "$cli" "$tmp/slash.transcript" slash
grep -a 'Enter/Tab complete' "$tmp/slash.transcript" >/dev/null
grep -a '/resume' "$tmp/slash.transcript" >/dev/null
python3 "$ROOT/tests/cli_pty_driver.py" "$cli" "$tmp/resize.transcript" resize
grep -a 'Connected' "$tmp/resize.transcript" >/dev/null
grep -a 'Commands' "$tmp/resize.transcript" >/dev/null
curl --fail --silent --show-error \
  -H "Authorization: Bearer $token" \
  "$url/v1/sessions?organization_id=org-e2e&team_id=team-e2e&actor_id=user-e2e" \
  > "$tmp/shared-sessions.json"
ruby -rjson -e '
  sessions = JSON.parse(File.read(ARGV.fetch(0)))
  titles = sessions.map { |session| session.fetch("title") }
  abort "shared daemon did not preserve both clients sessions" unless titles.sort == ["Terminal Team Session", "Web Shared Session"]
' "$tmp/shared-sessions.json"

# Non-interactive sandbox execution fails closed until the caller explicitly
# confirms the displayed profile and network request with --yes.
if "$cli" sandbox --sandbox-profile read-only --timeout 10 -- cargo --version \
  >"$tmp/sandbox.out" 2>"$tmp/sandbox.err"; then
  echo "sandbox command unexpectedly bypassed explicit confirmation" >&2
  exit 1
else
  exit_status=$?
fi
[ "$exit_status" -eq 1 ] || {
  echo "unconfirmed sandbox returned $exit_status instead of 1" >&2
  cat "$tmp/sandbox.err" >&2
  exit 1
}
[ ! -s "$tmp/sandbox.out" ]
grep -F 'sandbox execution requires confirmation; rerun with --yes' \
  "$tmp/sandbox.err" >/dev/null

# Once explicitly confirmed, the command uses the normal one-operation Policy
# approval, produces real output, and removes its disposable Session.
"$cli" sandbox --yes --sandbox-profile read-only --timeout 10 -- echo sandbox-ok \
  >"$tmp/sandbox-confirmed.out" 2>"$tmp/sandbox-confirmed.err"
grep -Fx 'sandbox-ok' "$tmp/sandbox-confirmed.out" >/dev/null
[ ! -s "$tmp/sandbox-confirmed.err" ]

# Network access is a separate explicit capability. This fixture does not need
# the network, but exercises the network-on Policy and approval path.
"$cli" sandbox --yes --network --sandbox-profile read-only --timeout 10 -- \
  echo network-capability-confirmed >"$tmp/sandbox-network.out" \
  2>"$tmp/sandbox-network.err"
grep -Fx 'network-capability-confirmed' "$tmp/sandbox-network.out" >/dev/null
[ ! -s "$tmp/sandbox-network.err" ]

if "$cli" sandbox --yes --sandbox-profile read-only --timeout 1 -- \
  python3 -c 'import time; time.sleep(5)' >"$tmp/sandbox-timeout.out" \
  2>"$tmp/sandbox-timeout.err"; then
  echo "sandbox timeout unexpectedly succeeded" >&2
  exit 1
else
  exit_status=$?
fi
[ "$exit_status" -eq 124 ] || {
  echo "sandbox timeout returned $exit_status instead of 124" >&2
  cat "$tmp/sandbox-timeout.err" >&2
  exit 1
}
grep -F 'process timed out' "$tmp/sandbox-timeout.err" >/dev/null
curl --fail --silent --show-error \
  -H "Authorization: Bearer $token" \
  "$url/v1/sessions?organization_id=org-e2e&team_id=team-e2e&actor_id=user-e2e" \
  > "$tmp/sessions-after-sandbox.json"
ruby -rjson -e '
  before = JSON.parse(File.read(ARGV.fetch(0))).map { |session| session.fetch("id") }.sort
  after = JSON.parse(File.read(ARGV.fetch(1))).map { |session| session.fetch("id") }.sort
  abort "sandbox disposable Session leaked" unless after == before
' "$tmp/shared-sessions.json" "$tmp/sessions-after-sandbox.json"

python3 "$ROOT/tests/cli_pty_driver.py" "$cli" "$tmp/agent.transcript" agent "$workspace"
grep -a 'Approval required' "$tmp/agent.transcript" >/dev/null
grep -a 'Enter confirm' "$tmp/agent.transcript" >/dev/null
grep -a 'Edit file · tracked.txt' "$tmp/agent.transcript" >/dev/null
grep -a 'write' "$tmp/agent.transcript" >/dev/null
grep -a 'Restored:' "$tmp/agent.transcript" >/dev/null
grep -a 'History search' "$tmp/agent.transcript" >/dev/null
grep -a 'pasted input inserted without subm' "$tmp/agent.transcript" >/dev/null
[ "$(sed -n '1p' "$workspace/tracked.txt")" = "original" ]

# CLI and Web restore the same typed transcript snapshot. Every visible item is
# permanently owned by one Session and Turn, and replay starts after its cursor.
ruby -rjson -e '
  sessions = JSON.parse(File.read(ARGV.fetch(0)))
  File.write(ARGV.fetch(1), sessions.map { |session| session.fetch("id") }.join("\n") + "\n")
' "$tmp/shared-sessions.json" "$tmp/session-ids"
: > "$tmp/snapshots.jsonl"
while IFS= read -r session_id; do
  curl --fail --silent --show-error \
    -H "Authorization: Bearer $token" \
    "$url/v1/sessions/$session_id/snapshot?organization_id=org-e2e&team_id=team-e2e&actor_id=user-e2e" \
    >> "$tmp/snapshots.jsonl"
  printf '\n' >> "$tmp/snapshots.jsonl"
done < "$tmp/session-ids"
ruby -rjson -e '
  snapshots = File.readlines(ARGV.fetch(0), chomp: true).reject(&:empty?).map { |line| JSON.parse(line) }
  abort "missing shared snapshots" unless snapshots.length == 2
  abort "snapshot cursor did not advance" unless snapshots.all? { |snapshot| snapshot.fetch("cursor") > 0 }
  snapshots.each do |snapshot|
    session_id = snapshot.fetch("session").fetch("id")
    turn_ids = snapshot.fetch("turns").map { |turn| turn.fetch("id") }
    ids = snapshot.fetch("items").map { |item| item.fetch("id") }
    abort "duplicate transcript item identity" unless ids.uniq == ids
    snapshot.fetch("items").each do |item|
      abort "cross-session transcript item" unless item.fetch("session_id") == session_id
      abort "orphan transcript item" unless turn_ids.include?(item.fetch("turn_id"))
    end
  end
  active = snapshots.max_by { |snapshot| snapshot.fetch("items").length }
  kinds = active.fetch("items").map { |item| item.fetch("kind") }
  abort "agent messages missing from typed transcript" unless kinds.include?("agent_message")
  abort "tool lifecycle missing from typed transcript" unless (kinds & ["tool_call", "file_change", "diff"]).any?
' "$tmp/snapshots.jsonl"

# Non-interactive output is stable for scripts and low-capability terminals:
# plain text only, no alternate screen, cursor controls, or animation.
NO_COLOR=1 TERM=dumb "$cli" --print --continue "Summarize the completed work." \
  >"$tmp/print.out"
grep -Fx 'write complete' "$tmp/print.out" >/dev/null
if LC_ALL=C grep "$(printf '\033')" "$tmp/print.out" >/dev/null; then
  echo "non-interactive CLI emitted terminal control characters" >&2
  exit 1
fi

# The public automation surfaces share the same daemon without entering the
# interactive screen. JSONL is versioned and the final message can be handed
# to another CI step without scraping terminal output.
"$cli" exec --jsonl --ephemeral --output-last-message "$tmp/last-message.txt" \
  "Summarize this workspace for automation." >"$tmp/exec.jsonl"
ruby -rjson -e '
  rows = File.readlines(ARGV.fetch(0), chomp: true).map { |line| JSON.parse(line) }
  abort "JSONL is not versioned" unless rows.all? { |row| row["schema_version"] == "1" }
  abort "missing turn start" unless rows.first["type"] == "turn.started"
  abort "missing message delta" unless rows.any? { |row| row["type"] == "message.delta" }
  abort "missing terminal record" unless rows.last["type"] == "turn.completed"
' "$tmp/exec.jsonl"
grep -Fx 'write complete' "$tmp/last-message.txt" >/dev/null

# Review defaults to an isolated read-only automation session. The fixture
# response is stable, while any write proposal would be rejected by print mode.
"$cli" review --uncommitted >"$tmp/review.out"
grep -Fx 'write complete' "$tmp/review.out" >/dev/null

"$cli" doctor >"$tmp/doctor.out"
grep -F 'daemon healthy' "$tmp/doctor.out" >/dev/null
grep -F 'storage encrypted with a private managed key' "$tmp/doctor.out" >/dev/null
grep -F 'local model endpoint catalog reachable; no credential required' "$tmp/doctor.out" >/dev/null
if grep -F 'credential accepted' "$tmp/doctor.out" >/dev/null; then
  echo "doctor claimed that a catalog request proved credential acceptance" >&2
  exit 1
fi
"$cli" completion zsh >"$tmp/completion.zsh"
grep -F '#compdef opencoding' "$tmp/completion.zsh" >/dev/null

python3 "$ROOT/tests/cli_pty_driver.py" "$cli" "$tmp/exit.transcript" exit

curl --fail --silent --show-error "http://$model_address/requests" \
  | ruby -rjson -e 'abort "interactive, exec, and review surfaces did not make five requests" unless JSON.parse(STDIN.read)["requests"] == 5'

echo "CLI/Web transcript, lifecycle, approval, sandbox fail-closed cleanup, diff, undo, history search, bounded paste, resize, exec, review, doctor and completion test passed"
