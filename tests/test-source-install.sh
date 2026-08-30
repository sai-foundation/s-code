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

for command in opencoding opencoding-cli opencoding-daemon; do
  [ -x "$INSTALL_DIR/$command" ]
done
[ ! -e "$INSTALL_DIR/opencoding-update" ]
"$INSTALL_DIR/opencoding" --help | grep -F 'opencoding web' >/dev/null
if "$INSTALL_DIR/opencoding" --help | grep -F 'opencoding update' >/dev/null; then
  echo "source-first launcher still advertises binary updates" >&2
  exit 1
fi
[ "$("$INSTALL_DIR/opencoding" cli smoke)" = 'opencoding-cli:smoke' ]
[ "$("$INSTALL_DIR/opencoding" web smoke)" = 'opencoding-daemon:smoke' ]

before="$(cksum "$INSTALL_DIR/opencoding-cli" "$INSTALL_DIR/opencoding-daemon")"
if SOURCE_INSTALL_LOG="$TASK/failure.log" \
  SOURCE_INSTALL_BUILD_FAIL=1 \
  PATH="$FAKEBIN:$PATH" \
  OPENCODING_INSTALL_DIR="$INSTALL_DIR" \
  CARGO_TARGET_DIR="$TARGET_DIR" \
    "$SOURCE/scripts/install-from-source.sh" > "$TASK/failure.stdout" 2> "$TASK/failure.stderr"; then
  echo "source installer accepted a failed build" >&2
  exit 1
fi
[ "$(cksum "$INSTALL_DIR/opencoding-cli" "$INSTALL_DIR/opencoding-daemon")" = "$before" ]

echo "source installation contract passed"
