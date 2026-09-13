#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
python3 "$ROOT/tests/test-source-dependencies.py"
python3 "$ROOT/tests/test-shell-path.py"
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
HOME="$TASK/home"
SHELL=/bin/bash
export HOME SHELL
unset ZDOTDIR XDG_CONFIG_HOME
mkdir -p "$HOME"
mkdir -p "$SOURCE/scripts" "$SOURCE/web" "$FAKEBIN"
if [ -f "$ROOT/release/community/scripts/install-from-source.sh" ]; then
  SOURCE_INSTALLER="$ROOT/release/community/scripts/install-from-source.sh"
else
  SOURCE_INSTALLER="$ROOT/scripts/install-from-source.sh"
fi
cp "$SOURCE_INSTALLER" "$SOURCE/scripts/install-from-source.sh"
cp "$ROOT/scripts/source-dependencies.sh" "$SOURCE/scripts/"
cp "$ROOT/scripts/configure-shell-path.py" "$SOURCE/scripts/"
cp "$ROOT/s-code" "$SOURCE/"
cp "$ROOT/rust-toolchain.toml" "$SOURCE/"
cp "$ROOT/scripts/s-code" "$SOURCE/scripts/"
printf '[workspace]\nmembers = []\n' > "$SOURCE/Cargo.toml"

cat > "$FAKEBIN/npm" <<'EOF'
#!/bin/sh
[ "${1:-}" != --version ] || { echo '10.9.8'; exit 0; }
if [ "${SOURCE_INSTALL_WAIT_ON_NPM_CI:-0}" = 1 ] && [ "${1:-}" = ci ]; then
  : > "$SOURCE_INSTALL_WAIT_MARKER"
  while [ ! -f "$SOURCE_INSTALL_WAIT_RELEASE" ]; do sleep 0.01; done
fi
printf 'npm %s\n' "$*" >> "$SOURCE_INSTALL_LOG"
EOF
cat > "$FAKEBIN/cargo" <<'EOF'
#!/bin/sh
[ "${1:-}" != --version ] || { echo 'cargo 1.89.0'; exit 0; }
printf 'cargo %s\n' "$*" >> "$SOURCE_INSTALL_LOG"
[ "${SOURCE_INSTALL_BUILD_FAIL:-0}" = 0 ] || exit 42
mkdir -p "$CARGO_TARGET_DIR/release"
for binary in s-code-daemon s-code-cli; do
  name="$binary"
  cat > "$CARGO_TARGET_DIR/release/$binary" <<SCRIPT
#!/bin/sh
if [ "\${1:-}" = "--self-test" ]; then exit 0; fi
if [ '${name}' = 's-code-cli' ] && [ "\${1:-}" = "--help" ]; then
  printf '%s\\n' 'S-Code CLI' 'Usage:' '  s-code [prompt]' \
    '  s-code exec [options] <prompt>' '  s-code setup' \
    '  s-code doctor' '  s-code sandbox -- <program>'
  exit 0
fi
if [ '${name}' = 's-code-daemon' ] && [ -n "\${S_CODE_DAEMON_TEST_MARKER:-}" ]; then
  : > "\$S_CODE_DAEMON_TEST_MARKER"
fi
if [ '${name}' = 's-code-cli' ] && [ "\${S_CODE_EXPECT_REMOTE_CONFIG:-0}" = 1 ]; then
  [ "\${S_CODE_URL:-}" = "\${S_CODE_EXPECT_URL:-}" ] || exit 98
  [ "\${S_CODE_TOKEN:-}" = "\${S_CODE_EXPECT_TOKEN:-}" ] || exit 99
fi
printf '${name}:%s\\n' "\$*"
SCRIPT
  chmod 0755 "$CARGO_TARGET_DIR/release/$binary"
done
EOF
# This contract tests installation transactions, not the platform sandbox.
# Keep its Linux preflight hermetic; real bubblewrap behavior is covered by
# test-cli-e2e.sh and test-privacy-security-use-cases.sh.
cat > "$FAKEBIN/bwrap" <<'EOF'
#!/bin/sh
exit 0
EOF
cat > "$FAKEBIN/node" <<'EOF'
#!/bin/sh
exit 0
EOF
cat > "$FAKEBIN/rustc" <<'EOF'
#!/bin/sh
echo 'rustc 1.89.0 (fixture)'
EOF
chmod 0755 "$FAKEBIN/npm" "$FAKEBIN/cargo" "$FAKEBIN/bwrap" "$FAKEBIN/node" "$FAKEBIN/rustc"

SOURCE_INSTALL_LOG="$TASK/install.log" \
PATH="$FAKEBIN:$PATH" \
S_CODE_INSTALL_DIR="$INSTALL_DIR" \
CARGO_TARGET_DIR="$TARGET_DIR" \
  "$SOURCE/scripts/install-from-source.sh" > "$TASK/stdout"

grep -F "npm ci --prefix $SOURCE/web --no-audit --no-fund" "$TASK/install.log" >/dev/null
grep -F "npm run build --prefix $SOURCE/web" "$TASK/install.log" >/dev/null
grep -F "cargo build --locked --release --manifest-path $SOURCE/Cargo.toml -p s-code-daemon -p s-code-cli" \
  "$TASK/install.log" >/dev/null
grep -F "Installed S-Code from source to $INSTALL_DIR" "$TASK/stdout" >/dev/null
grep -F "New terminals can run: s-code" "$TASK/stdout" >/dev/null
grep -F "From this checkout you can also run: ./s-code" "$TASK/stdout" >/dev/null
grep -F "$INSTALL_DIR/s-code restart" "$TASK/stdout" >/dev/null
[ -f "$HOME/.bashrc" ]
[ -f "$HOME/.bash_profile" ]
S_CODE_INSTALL_DIR="$INSTALL_DIR" "$SOURCE/s-code" --help | grep -F 's-code setup' >/dev/null

for command in s-code s-code-cli s-code-daemon; do
  [ -x "$INSTALL_DIR/$command" ]
  [ -L "$INSTALL_DIR/$command" ]
done
[ -L "$INSTALL_DIR/.s-code-source-current" ]
first_target="$(readlink "$INSTALL_DIR/.s-code-source-current")"
case "$first_target" in
  .s-code-source-releases/release.*) ;;
  *) echo "first install did not select its qualified release" >&2; exit 1 ;;
esac
[ ! -e "$INSTALL_DIR/s-code-update" ]
"$INSTALL_DIR/s-code" --help | grep -F 's-code web' >/dev/null
"$INSTALL_DIR/s-code" --help | grep -F 's-code setup' >/dev/null
"$INSTALL_DIR/s-code" --help | grep -F 's-code doctor' >/dev/null
"$INSTALL_DIR/s-code" --help | grep -F 's-code [prompt]' >/dev/null
"$INSTALL_DIR/s-code" --help | grep -F 's-code exec' >/dev/null
"$INSTALL_DIR/s-code" --help | grep -F 's-code sandbox' >/dev/null
if "$INSTALL_DIR/s-code" --help | grep -F 's-code update' >/dev/null; then
  echo "source-first launcher still advertises binary updates" >&2
  exit 1
fi
[ "$(S_CODE_NO_AUTOSTART=1 "$INSTALL_DIR/s-code" cli smoke)" = 's-code-cli:smoke' ]
[ "$("$INSTALL_DIR/s-code" web smoke)" = 's-code-daemon:smoke' ]

# An explicit remote automation connection must go directly to the CLI. It
# must not start a local daemon, reuse the remote bearer credential locally,
# or create local runtime, state, or log files first.
remote_home="$TASK/remote-home"
daemon_marker="$TASK/remote-daemon-started"
remote_url="https://daemon.example.invalid"
remote_token="fixture-remote-token"
remote_output="$(
  S_CODE_HOME="$remote_home" \
  S_CODE_URL="$remote_url" \
  S_CODE_TOKEN="$remote_token" \
  S_CODE_EXPECT_REMOTE_CONFIG=1 \
  S_CODE_EXPECT_URL="$remote_url" \
  S_CODE_EXPECT_TOKEN="$remote_token" \
  S_CODE_DAEMON_TEST_MARKER="$daemon_marker" \
    "$INSTALL_DIR/s-code" cli remote
)"
[ "$remote_output" = 's-code-cli:remote' ]
[ ! -e "$daemon_marker" ]
[ ! -e "$remote_home" ]
remote_default_output="$(
  S_CODE_HOME="$remote_home" \
  S_CODE_URL="$remote_url" \
  S_CODE_TOKEN="$remote_token" \
  S_CODE_EXPECT_REMOTE_CONFIG=1 \
  S_CODE_EXPECT_URL="$remote_url" \
  S_CODE_EXPECT_TOKEN="$remote_token" \
  S_CODE_DAEMON_TEST_MARKER="$daemon_marker" \
    "$INSTALL_DIR/s-code" remote-default
)"
[ "$remote_default_output" = 's-code-cli:remote-default' ]
[ ! -e "$daemon_marker" ]
[ ! -e "$remote_home" ]

# Invalid directory overrides fail before the launcher creates anything.
path_probe="$TASK/path-probe"
mkdir -p "$path_probe"
if (cd "$path_probe" && S_CODE_HOME=relative-home "$INSTALL_DIR/s-code" cli probe \
    >"$TASK/relative-home.out" 2>"$TASK/relative-home.err"); then
  echo "launcher accepted a relative S_CODE_HOME" >&2
  exit 1
fi
[ ! -e "$path_probe/relative-home" ]
if (cd "$path_probe" && \
    S_CODE_HOME="$TASK/valid-home" \
    S_CODE_RUNTIME_DIR=relative-runtime \
    "$INSTALL_DIR/s-code" cli probe \
    >"$TASK/relative-runtime.out" 2>"$TASK/relative-runtime.err"); then
  echo "launcher accepted a relative S_CODE_RUNTIME_DIR" >&2
  exit 1
fi
[ ! -e "$path_probe/relative-runtime" ]
[ ! -e "$TASK/valid-home" ]
if (cd "$path_probe" && \
    S_CODE_HOME="$TASK/valid-home" \
    S_CODE_STATE_DIR="$path_probe/../escaped-state" \
    "$INSTALL_DIR/s-code" cli probe \
    >"$TASK/parent-state.out" 2>"$TASK/parent-state.err"); then
  echo "launcher accepted an S_CODE_STATE_DIR containing .." >&2
  exit 1
fi
[ ! -e "$TASK/escaped-state" ]
[ ! -e "$TASK/valid-home" ]

# BSD mv follows a destination symlink to a directory. An upgrade must replace
# even that malformed public link rather than moving its temporary link inside
# the directory and continuing to execute the stale target.
mkdir "$INSTALL_DIR/anomalous-link-target"
rm "$INSTALL_DIR/s-code-cli"
ln -s "anomalous-link-target" "$INSTALL_DIR/s-code-cli"
SOURCE_INSTALL_LOG="$TASK/anomalous-link.log" \
PATH="$FAKEBIN:$PATH" \
S_CODE_INSTALL_DIR="$INSTALL_DIR" \
CARGO_TARGET_DIR="$TARGET_DIR" \
  "$SOURCE/scripts/install-from-source.sh" > "$TASK/anomalous-link.stdout"
[ "$(readlink "$INSTALL_DIR/s-code-cli")" = '.s-code-source-current/s-code-cli' ]
if find "$INSTALL_DIR/anomalous-link-target" -name '.s-code-cli.link.*' -print | grep . >/dev/null; then
  echo "source installer moved a replacement link inside the old symlink target" >&2
  exit 1
fi

before="$(cksum "$INSTALL_DIR/s-code" "$INSTALL_DIR/s-code-cli" "$INSTALL_DIR/s-code-daemon")"
if SOURCE_INSTALL_LOG="$TASK/failure.log" \
  SOURCE_INSTALL_BUILD_FAIL=1 \
  PATH="$FAKEBIN:$PATH" \
  S_CODE_INSTALL_DIR="$INSTALL_DIR" \
  CARGO_TARGET_DIR="$TARGET_DIR" \
    "$SOURCE/scripts/install-from-source.sh" > "$TASK/failure.stdout" 2> "$TASK/failure.stderr"; then
  echo "source installer accepted a failed build" >&2
  exit 1
fi
[ "$(cksum "$INSTALL_DIR/s-code" "$INSTALL_DIR/s-code-cli" "$INSTALL_DIR/s-code-daemon")" = "$before" ]

marker="$TASK/install-checkpoint"
SOURCE_INSTALL_LOG="$TASK/killed.log" \
S_CODE_INSTALL_TEST_CHECKPOINT=after-release-ready \
S_CODE_INSTALL_TEST_MARKER="$marker" \
PATH="$FAKEBIN:$PATH" \
S_CODE_INSTALL_DIR="$INSTALL_DIR" \
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
[ "$(cksum "$INSTALL_DIR/s-code" "$INSTALL_DIR/s-code-cli" "$INSTALL_DIR/s-code-daemon")" = "$before" ]

# A subsequent run takes over the stale lock, removes the unpublished release
# and leaves a complete suite behind.
SOURCE_INSTALL_LOG="$TASK/recovery.log" \
PATH="$FAKEBIN:$PATH" \
S_CODE_INSTALL_DIR="$INSTALL_DIR" \
CARGO_TARGET_DIR="$TARGET_DIR" \
  "$SOURCE/scripts/install-from-source.sh" > "$TASK/recovery.stdout"
recovery_target="$(readlink "$INSTALL_DIR/.s-code-source-current")"
case "$recovery_target" in
  .s-code-source-releases/release.*) ;;
  *) echo "recovery install did not select its qualified release" >&2; exit 1 ;;
esac
[ "$recovery_target" != "$first_target" ] || {
  echo "successful upgrade did not switch the installed release" >&2
  exit 1
}
for command in s-code s-code-cli s-code-daemon; do
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
S_CODE_INSTALL_DIR="$INSTALL_DIR" \
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
S_CODE_INSTALL_DIR="$INSTALL_DIR" \
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
"$INSTALL_DIR/s-code-daemon" --self-test
"$INSTALL_DIR/s-code-cli" --self-test
"$INSTALL_DIR/s-code" --help >/dev/null

if find "$INSTALL_DIR" -name '.s-code-source-current.next.*' -print | grep . >/dev/null; then
  echo "source installer misplaced a current-pointer link inside a release" >&2
  exit 1
fi

if find "$INSTALL_DIR" -maxdepth 1 \
  \( -name '.s-code-source-stage.*' -o -name '.s-code-source-backup.*' \) \
  -print | grep . >/dev/null; then
  echo "source installer left transaction artifacts behind" >&2
  exit 1
fi

# Automation can combine dependency and PATH options without changing profiles.
opt_out_home="$TASK/opt-out-home"
mkdir -p "$opt_out_home"
HOME="$opt_out_home" \
SOURCE_INSTALL_LOG="$TASK/opt-out.log" \
PATH="$FAKEBIN:$PATH" \
S_CODE_INSTALL_DIR="$INSTALL_DIR" \
CARGO_TARGET_DIR="$TARGET_DIR" \
  "$SOURCE/scripts/install-from-source.sh" --no-install-deps --no-modify-path > "$TASK/opt-out.stdout"
[ ! -e "$opt_out_home/.bashrc" ]
[ ! -e "$opt_out_home/.bash_profile" ]
grep -F "Start now (works without changing PATH):" "$TASK/opt-out.stdout" >/dev/null

echo "source installation contract passed"
