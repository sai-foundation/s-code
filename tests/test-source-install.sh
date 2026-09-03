#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
mkdir -p "$ROOT/.work"
TASK="$(mktemp -d "$ROOT/.work/source-install.XXXXXX")"
cleanup() {
  status=$?
  trap - EXIT HUP INT TERM
  if [ "$status" -eq 0 ]; then
    find "$TASK" -depth -delete
  else
    echo "source installation test failed; retained $TASK" >&2
  fi
  exit "$status"
}
trap cleanup EXIT HUP INT TERM

SOURCE="$TASK/source"
FAKEBIN="$TASK/fakebin"
INSTALL_DIR="$TASK/install"
TARGET_DIR="$TASK/target"
mkdir -p "$SOURCE/scripts" "$SOURCE/web" "$FAKEBIN"
if [ -f "$ROOT/release/community/scripts/install-from-source.sh" ]; then
  SOURCE_INSTALLER="$ROOT/release/community/scripts/install-from-source.sh"
else
  SOURCE_INSTALLER="$ROOT/scripts/install-from-source.sh"
fi
cp "$SOURCE_INSTALLER" "$SOURCE/scripts/install-from-source.sh"
cp "$ROOT/scripts/opencoding" "$SOURCE/scripts/"
printf '[workspace]\nmembers = []\n' > "$SOURCE/Cargo.toml"

cat > "$FAKEBIN/npm" <<'EOF'
#!/bin/sh
if [ "${SOURCE_INSTALL_WAIT_ON_NPM_CI:-0}" = 1 ] && [ "${1:-}" = ci ]; then
  : > "$SOURCE_INSTALL_WAIT_MARKER"
  while [ ! -f "$SOURCE_INSTALL_WAIT_RELEASE" ]; do sleep 0.01; done
fi
printf 'npm %s\n' "$*" >> "$SOURCE_INSTALL_LOG"
EOF
cat > "$FAKEBIN/cargo" <<'EOF'
#!/bin/sh
printf 'cargo %s\n' "$*" >> "$SOURCE_INSTALL_LOG"
[ "${SOURCE_INSTALL_BUILD_FAIL:-0}" = 0 ] || exit 42
mkdir -p "$CARGO_TARGET_DIR/release"
for binary in opencoding-daemon opencoding-cli; do
  name="$binary"
  cat > "$CARGO_TARGET_DIR/release/$binary" <<SCRIPT
#!/bin/sh
if [ "\${1:-}" = "--self-test" ]; then exit 0; fi
if [ '${name}' = 'opencoding-cli' ] && [ "\${1:-}" = "--help" ]; then
  printf '%s\\n' 'Opencoding CLI' 'Usage:' '  opencoding [prompt]' \
    '  opencoding exec [options] <prompt>' '  opencoding setup' \
    '  opencoding doctor' '  opencoding sandbox -- <program>'
  exit 0
fi
if [ '${name}' = 'opencoding-daemon' ] && [ -n "\${OPENCODING_DAEMON_TEST_MARKER:-}" ]; then
  : > "\$OPENCODING_DAEMON_TEST_MARKER"
fi
if [ '${name}' = 'opencoding-cli' ] && [ "\${OPENCODING_EXPECT_REMOTE_CONFIG:-0}" = 1 ]; then
  [ "\${OPENCODING_URL:-}" = "\${OPENCODING_EXPECT_URL:-}" ] || exit 98
  [ "\${OPENCODING_TOKEN:-}" = "\${OPENCODING_EXPECT_TOKEN:-}" ] || exit 99
fi
printf '${name}:%s\\n' "\$*"
SCRIPT
  chmod 0755 "$CARGO_TARGET_DIR/release/$binary"
done
EOF
cat > "$FAKEBIN/rg" <<'EOF'
#!/bin/sh
exit 0
EOF
chmod 0755 "$FAKEBIN/npm" "$FAKEBIN/cargo" "$FAKEBIN/rg"

SOURCE_INSTALL_LOG="$TASK/install.log" \
PATH="$FAKEBIN:$PATH" \
OPENCODING_INSTALL_DIR="$INSTALL_DIR" \
CARGO_TARGET_DIR="$TARGET_DIR" \
  "$SOURCE/scripts/install-from-source.sh" > "$TASK/stdout"

grep -F "npm ci --prefix $SOURCE/web" "$TASK/install.log" >/dev/null
grep -F "npm run build --prefix $SOURCE/web" "$TASK/install.log" >/dev/null
grep -F "cargo build --locked --release --manifest-path $SOURCE/Cargo.toml -p opencoding-daemon -p opencoding-cli" \
  "$TASK/install.log" >/dev/null
grep -F "Installed Opencoding Community from source to $INSTALL_DIR" "$TASK/stdout" >/dev/null
grep -F "A running Opencoding service is not restarted automatically" "$TASK/stdout" >/dev/null

for command in opencoding opencoding-cli opencoding-daemon; do
  [ -x "$INSTALL_DIR/$command" ]
  [ -L "$INSTALL_DIR/$command" ]
done
[ -L "$INSTALL_DIR/.opencoding-source-current" ]
first_target="$(readlink "$INSTALL_DIR/.opencoding-source-current")"
case "$first_target" in
  .opencoding-source-releases/release.*) ;;
  *) echo "first install did not select its qualified release" >&2; exit 1 ;;
esac
[ ! -e "$INSTALL_DIR/opencoding-update" ]
"$INSTALL_DIR/opencoding" --help | grep -F 'opencoding web' >/dev/null
"$INSTALL_DIR/opencoding" --help | grep -F 'opencoding setup' >/dev/null
"$INSTALL_DIR/opencoding" --help | grep -F 'opencoding doctor' >/dev/null
"$INSTALL_DIR/opencoding" --help | grep -F 'opencoding [prompt]' >/dev/null
"$INSTALL_DIR/opencoding" --help | grep -F 'opencoding exec' >/dev/null
"$INSTALL_DIR/opencoding" --help | grep -F 'opencoding sandbox' >/dev/null
if "$INSTALL_DIR/opencoding" --help | grep -F 'opencoding update' >/dev/null; then
  echo "source-first launcher still advertises binary updates" >&2
  exit 1
fi
[ "$(OPENCODING_NO_AUTOSTART=1 "$INSTALL_DIR/opencoding" cli smoke)" = 'opencoding-cli:smoke' ]
[ "$("$INSTALL_DIR/opencoding" web smoke)" = 'opencoding-daemon:smoke' ]

# An explicit remote automation connection must go directly to the CLI. It
# must not start a local daemon, reuse the remote bearer credential locally,
# or create local runtime, state, or log files first.
remote_home="$TASK/remote-home"
daemon_marker="$TASK/remote-daemon-started"
remote_url="https://daemon.example.invalid"
remote_token="fixture-remote-token"
remote_output="$(
  OPENCODING_HOME="$remote_home" \
  OPENCODING_URL="$remote_url" \
  OPENCODING_TOKEN="$remote_token" \
  OPENCODING_EXPECT_REMOTE_CONFIG=1 \
  OPENCODING_EXPECT_URL="$remote_url" \
  OPENCODING_EXPECT_TOKEN="$remote_token" \
  OPENCODING_DAEMON_TEST_MARKER="$daemon_marker" \
    "$INSTALL_DIR/opencoding" cli remote
)"
[ "$remote_output" = 'opencoding-cli:remote' ]
[ ! -e "$daemon_marker" ]
[ ! -e "$remote_home" ]
remote_default_output="$(
  OPENCODING_HOME="$remote_home" \
  OPENCODING_URL="$remote_url" \
  OPENCODING_TOKEN="$remote_token" \
  OPENCODING_EXPECT_REMOTE_CONFIG=1 \
  OPENCODING_EXPECT_URL="$remote_url" \
  OPENCODING_EXPECT_TOKEN="$remote_token" \
  OPENCODING_DAEMON_TEST_MARKER="$daemon_marker" \
    "$INSTALL_DIR/opencoding" remote-default
)"
[ "$remote_default_output" = 'opencoding-cli:remote-default' ]
[ ! -e "$daemon_marker" ]
[ ! -e "$remote_home" ]

# Invalid directory overrides fail before the launcher creates anything.
path_probe="$TASK/path-probe"
mkdir -p "$path_probe"
if (cd "$path_probe" && OPENCODING_HOME=relative-home "$INSTALL_DIR/opencoding" cli probe \
    >"$TASK/relative-home.out" 2>"$TASK/relative-home.err"); then
  echo "launcher accepted a relative OPENCODING_HOME" >&2
  exit 1
fi
[ ! -e "$path_probe/relative-home" ]
if (cd "$path_probe" && \
    OPENCODING_HOME="$TASK/valid-home" \
    OPENCODING_RUNTIME_DIR=relative-runtime \
    "$INSTALL_DIR/opencoding" cli probe \
    >"$TASK/relative-runtime.out" 2>"$TASK/relative-runtime.err"); then
  echo "launcher accepted a relative OPENCODING_RUNTIME_DIR" >&2
  exit 1
fi
[ ! -e "$path_probe/relative-runtime" ]
[ ! -e "$TASK/valid-home" ]
if (cd "$path_probe" && \
    OPENCODING_HOME="$TASK/valid-home" \
    OPENCODING_STATE_DIR="$path_probe/../escaped-state" \
    "$INSTALL_DIR/opencoding" cli probe \
    >"$TASK/parent-state.out" 2>"$TASK/parent-state.err"); then
  echo "launcher accepted an OPENCODING_STATE_DIR containing .." >&2
  exit 1
fi
[ ! -e "$TASK/escaped-state" ]
[ ! -e "$TASK/valid-home" ]

# BSD mv follows a destination symlink to a directory. An upgrade must replace
# even that malformed public link rather than moving its temporary link inside
# the directory and continuing to execute the stale target.
mkdir "$INSTALL_DIR/anomalous-link-target"
rm "$INSTALL_DIR/opencoding-cli"
ln -s "anomalous-link-target" "$INSTALL_DIR/opencoding-cli"
SOURCE_INSTALL_LOG="$TASK/anomalous-link.log" \
PATH="$FAKEBIN:$PATH" \
OPENCODING_INSTALL_DIR="$INSTALL_DIR" \
CARGO_TARGET_DIR="$TARGET_DIR" \
  "$SOURCE/scripts/install-from-source.sh" > "$TASK/anomalous-link.stdout"
[ "$(readlink "$INSTALL_DIR/opencoding-cli")" = '.opencoding-source-current/opencoding-cli' ]
if find "$INSTALL_DIR/anomalous-link-target" -name '.opencoding-cli.link.*' -print | grep . >/dev/null; then
  echo "source installer moved a replacement link inside the old symlink target" >&2
  exit 1
fi

before="$(cksum "$INSTALL_DIR/opencoding" "$INSTALL_DIR/opencoding-cli" "$INSTALL_DIR/opencoding-daemon")"
if SOURCE_INSTALL_LOG="$TASK/failure.log" \
  SOURCE_INSTALL_BUILD_FAIL=1 \
  PATH="$FAKEBIN:$PATH" \
  OPENCODING_INSTALL_DIR="$INSTALL_DIR" \
  CARGO_TARGET_DIR="$TARGET_DIR" \
    "$SOURCE/scripts/install-from-source.sh" > "$TASK/failure.stdout" 2> "$TASK/failure.stderr"; then
  echo "source installer accepted a failed build" >&2
  exit 1
fi
[ "$(cksum "$INSTALL_DIR/opencoding" "$INSTALL_DIR/opencoding-cli" "$INSTALL_DIR/opencoding-daemon")" = "$before" ]

marker="$TASK/install-checkpoint"
SOURCE_INSTALL_LOG="$TASK/killed.log" \
OPENCODING_INSTALL_TEST_CHECKPOINT=after-release-ready \
OPENCODING_INSTALL_TEST_MARKER="$marker" \
PATH="$FAKEBIN:$PATH" \
OPENCODING_INSTALL_DIR="$INSTALL_DIR" \
CARGO_TARGET_DIR="$TARGET_DIR" \
  "$SOURCE/scripts/install-from-source.sh" > "$TASK/killed.stdout" 2> "$TASK/killed.stderr" &
killed_pid=$!
attempts=0
while [ ! -f "$marker" ] && [ "$attempts" -lt 200 ]; do
  attempts=$((attempts + 1))
  sleep 0.01
done
[ -f "$marker" ] || { echo "installer did not reach crash checkpoint" >&2; exit 1; }
kill -KILL "$killed_pid"
wait "$killed_pid" 2>/dev/null || true
[ "$(cksum "$INSTALL_DIR/opencoding" "$INSTALL_DIR/opencoding-cli" "$INSTALL_DIR/opencoding-daemon")" = "$before" ]

# A subsequent run takes over the stale lock, removes the unpublished release
# and leaves a complete suite behind.
SOURCE_INSTALL_LOG="$TASK/recovery.log" \
PATH="$FAKEBIN:$PATH" \
OPENCODING_INSTALL_DIR="$INSTALL_DIR" \
CARGO_TARGET_DIR="$TARGET_DIR" \
  "$SOURCE/scripts/install-from-source.sh" > "$TASK/recovery.stdout"
recovery_target="$(readlink "$INSTALL_DIR/.opencoding-source-current")"
case "$recovery_target" in
  .opencoding-source-releases/release.*) ;;
  *) echo "recovery install did not select its qualified release" >&2; exit 1 ;;
esac
[ "$recovery_target" != "$first_target" ] || {
  echo "successful upgrade did not switch the installed release" >&2
  exit 1
}
for command in opencoding opencoding-cli opencoding-daemon; do
  [ -x "$INSTALL_DIR/$command" ]
done

# Concurrent installs serialize before they mutate shared npm/Cargo output or
# the publication pointer.
wait_marker="$TASK/concurrent-first-started"
wait_release="$TASK/concurrent-first-release"
SOURCE_INSTALL_LOG="$TASK/concurrent-first.log" \
SOURCE_INSTALL_WAIT_ON_NPM_CI=1 \
SOURCE_INSTALL_WAIT_MARKER="$wait_marker" \
SOURCE_INSTALL_WAIT_RELEASE="$wait_release" \
PATH="$FAKEBIN:$PATH" \
OPENCODING_INSTALL_DIR="$INSTALL_DIR" \
CARGO_TARGET_DIR="$TARGET_DIR" \
  "$SOURCE/scripts/install-from-source.sh" > "$TASK/concurrent-first.stdout" &
first_pid=$!
attempts=0
while [ ! -f "$wait_marker" ] && [ "$attempts" -lt 200 ]; do
  attempts=$((attempts + 1))
  sleep 0.01
done
[ -f "$wait_marker" ] || { echo "first concurrent installer did not start" >&2; exit 1; }
SOURCE_INSTALL_LOG="$TASK/concurrent-second.log" \
PATH="$FAKEBIN:$PATH" \
OPENCODING_INSTALL_DIR="$INSTALL_DIR" \
CARGO_TARGET_DIR="$TARGET_DIR" \
  "$SOURCE/scripts/install-from-source.sh" > "$TASK/concurrent-second.stdout" &
second_pid=$!
sleep 0.1
[ ! -s "$TASK/concurrent-second.log" ] || {
  echo "second installer entered the build while the first held the lock" >&2
  exit 1
}
: > "$wait_release"
wait "$first_pid"
wait "$second_pid"
"$INSTALL_DIR/opencoding-daemon" --self-test
"$INSTALL_DIR/opencoding-cli" --self-test
"$INSTALL_DIR/opencoding" --help >/dev/null

if find "$INSTALL_DIR" -name '.opencoding-source-current.next.*' -print | grep . >/dev/null; then
  echo "source installer misplaced a current-pointer link inside a release" >&2
  exit 1
fi

if find "$INSTALL_DIR" -maxdepth 1 \
  \( -name '.opencoding-source-stage.*' -o -name '.opencoding-source-backup.*' \) \
  -print | grep . >/dev/null; then
  echo "source installer left transaction artifacts behind" >&2
  exit 1
fi

echo "source installation contract passed"
