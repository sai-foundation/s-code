#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
INSTALL_DIR="${OPENCODING_INSTALL_DIR:-$HOME/.local/bin}"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target/opencoding-community-source}"

case "$INSTALL_DIR" in
  /|"$HOME"|"$ROOT")
    echo "install destination must be a dedicated binary directory" >&2
    exit 2
    ;;
esac
case "$TARGET_DIR" in
  /|"$HOME"|"$ROOT")
    echo "Cargo target must be a dedicated build directory" >&2
    exit 2
    ;;
esac

command -v cargo >/dev/null 2>&1 || { echo "cargo is required" >&2; exit 1; }
command -v npm >/dev/null 2>&1 || { echo "npm is required" >&2; exit 1; }
command -v rg >/dev/null 2>&1 || { echo "ripgrep (rg) is required at runtime" >&2; exit 1; }
command -v install >/dev/null 2>&1 || { echo "install is required" >&2; exit 1; }

export CARGO_TARGET_DIR="$TARGET_DIR"
npm ci --prefix "$ROOT/web"
npm run build --prefix "$ROOT/web"
cargo build --locked --release --manifest-path "$ROOT/Cargo.toml" \
  -p opencoding-daemon -p opencoding-cli

mkdir -p "$INSTALL_DIR"
staged=""
cleanup() {
  [ -z "$staged" ] || [ ! -e "$staged" ] || rm -f "$staged"
}
trap cleanup EXIT HUP INT TERM

for binary in opencoding-daemon opencoding-cli; do
  source_file="$TARGET_DIR/release/$binary"
  [ -x "$source_file" ] || { echo "build did not produce $binary" >&2; exit 1; }
  staged="$INSTALL_DIR/.${binary}.source-new.$$"
  install -m 0755 "$source_file" "$staged"
  mv -f "$staged" "$INSTALL_DIR/$binary"
  staged=""
done
for mapping in "opencoding:scripts/opencoding" "opencoding-update:scripts/update.sh"; do
  destination="${mapping%%:*}"
  source_relative="${mapping#*:}"
  staged="$INSTALL_DIR/.${destination}.source-new.$$"
  install -m 0755 "$ROOT/$source_relative" "$staged"
  mv -f "$staged" "$INSTALL_DIR/$destination"
  staged=""
done
trap - EXIT HUP INT TERM

"$INSTALL_DIR/opencoding-daemon" --self-test >/dev/null
"$INSTALL_DIR/opencoding-cli" --self-test >/dev/null
"$INSTALL_DIR/opencoding" --help >/dev/null

echo "Installed Opencoding Community from source to $INSTALL_DIR"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "Add $INSTALL_DIR to PATH before running opencoding." ;;
esac
