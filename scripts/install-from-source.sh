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
command -v python3 >/dev/null 2>&1 || { echo "python3 is required" >&2; exit 1; }
if [ "$(uname -s)" = "Linux" ]; then
  command -v bwrap >/dev/null 2>&1 || {
    echo "Bubblewrap (bwrap) is required for Linux command isolation" >&2
    exit 1
  }
fi

mkdir -p "$INSTALL_DIR"
LOCK_FILE="$INSTALL_DIR/.opencoding-source-install.lock"
if [ "${OPENCODING_INSTALL_LOCK_HELD:-0}" != 1 ]; then
  exec python3 -c '
import fcntl, os, stat, sys
script, lock, *arguments = sys.argv[1:]
flags = os.O_CREAT | os.O_RDWR
if hasattr(os, "O_NOFOLLOW"):
    flags |= os.O_NOFOLLOW
descriptor = os.open(lock, flags, 0o600)
if not stat.S_ISREG(os.fstat(descriptor).st_mode):
    raise SystemExit("source installation lock must be a regular file")
fcntl.flock(descriptor, fcntl.LOCK_EX)
os.set_inheritable(descriptor, True)
environment = os.environ.copy()
environment["OPENCODING_INSTALL_LOCK_HELD"] = "1"
os.execve(script, [script, *arguments], environment)
' "$0" "$LOCK_FILE" "$@"
fi
RELEASES_DIR="$INSTALL_DIR/.opencoding-source-releases"
CURRENT_LINK="$INSTALL_DIR/.opencoding-source-current"
STAGE_DIR=""
RELEASE_DIR=""

test_checkpoint() {
  [ "${OPENCODING_INSTALL_TEST_CHECKPOINT:-}" = "$1" ] || return 0
  [ -n "${OPENCODING_INSTALL_TEST_MARKER:-}" ] || return 0
  printf '%s\n' "$1" > "$OPENCODING_INSTALL_TEST_MARKER"
  kill -STOP "$$"
}

cleanup() {
  status=$?
  trap - EXIT HUP INT TERM
  if [ -n "$STAGE_DIR" ] && [ -d "$STAGE_DIR" ]; then
    find "$STAGE_DIR" -depth -delete 2>/dev/null || true
  fi
  if [ -n "$RELEASE_DIR" ] && [ -d "$RELEASE_DIR" ]; then
    current_target="$(readlink "$CURRENT_LINK" 2>/dev/null || true)"
    case "$RELEASE_DIR" in
      "$INSTALL_DIR"/*) release_relative="${RELEASE_DIR#"$INSTALL_DIR/"}" ;;
      *) release_relative="" ;;
    esac
    [ "$current_target" = "$release_relative" ] ||
      find "$RELEASE_DIR" -depth -delete 2>/dev/null || true
  fi
  exit "$status"
}
trap cleanup EXIT HUP INT TERM

mkdir -p "$RELEASES_DIR"
# A killed installer can only leave an unpublished staging directory. It is
# never referenced by the public commands, so the next exclusive installer can
# safely remove it before building a new release.
find "$RELEASES_DIR" -mindepth 1 -maxdepth 1 -type d -name '.stage.*' \
  -exec sh -c 'for path do find "$path" -depth -delete; done' sh {} +
current_target="$(readlink "$CURRENT_LINK" 2>/dev/null || true)"
for old_release in "$RELEASES_DIR"/release.*; do
  [ -d "$old_release" ] || continue
  old_relative="${old_release#"$INSTALL_DIR/"}"
  [ "$old_relative" = "$current_target" ] || find "$old_release" -depth -delete
done

export CARGO_TARGET_DIR="$TARGET_DIR"
npm ci --prefix "$ROOT/web" --no-audit --no-fund
npm run build --prefix "$ROOT/web"
cargo build --locked --release --manifest-path "$ROOT/Cargo.toml" \
  -p opencoding-daemon -p opencoding-cli

STAGE_DIR="$(mktemp -d "$RELEASES_DIR/.stage.XXXXXX")"

for binary in opencoding-daemon opencoding-cli; do
  source_file="$TARGET_DIR/release/$binary"
  [ -x "$source_file" ] || { echo "build did not produce $binary" >&2; exit 1; }
  install -m 0755 "$source_file" "$STAGE_DIR/$binary"
done
for mapping in "opencoding:scripts/opencoding"; do
  destination="${mapping%%:*}"
  source_relative="${mapping#*:}"
  install -m 0755 "$ROOT/$source_relative" "$STAGE_DIR/$destination"
done

# Qualify the complete staged suite before touching an installed command.
"$STAGE_DIR/opencoding-daemon" --self-test >/dev/null
"$STAGE_DIR/opencoding-cli" --self-test >/dev/null
"$STAGE_DIR/opencoding" --help >/dev/null

# Publish an immutable, already-qualified release directory. All three public
# commands resolve through one pointer, so the final pointer rename switches
# the suite in a single filesystem operation.
stage_name="${STAGE_DIR##*/}"
RELEASE_DIR="$RELEASES_DIR/release.${stage_name#.stage.}"
mv "$STAGE_DIR" "$RELEASE_DIR"
STAGE_DIR=""
release_relative="${RELEASE_DIR#"$INSTALL_DIR/"}"
test_checkpoint after-release-ready

# Repair or create stable public links before switching the current pointer.
# On upgrades they continue to resolve to the previous release until the final
# rename. A first-install crash can leave only links to a qualified release;
# rerunning the installer completes the same recoverable layout.
current_target="$(readlink "$CURRENT_LINK" 2>/dev/null || true)"
if [ ! -L "$CURRENT_LINK" ] || [ ! -d "$INSTALL_DIR/$current_target" ]; then
  initial_relative="$release_relative"
  existing=0
  for name in opencoding-daemon opencoding-cli opencoding; do
    [ -e "$INSTALL_DIR/$name" ] || [ -L "$INSTALL_DIR/$name" ] || continue
    existing=$((existing + 1))
  done
  if [ "$existing" -gt 0 ]; then
    PREVIOUS_STAGE="$(mktemp -d "$RELEASES_DIR/.stage.previous.XXXXXX")"
    for name in opencoding-daemon opencoding-cli opencoding; do
      if [ -x "$INSTALL_DIR/$name" ] && [ ! -L "$INSTALL_DIR/$name" ]; then
        install -m 0755 "$INSTALL_DIR/$name" "$PREVIOUS_STAGE/$name"
      else
        install -m 0755 "$RELEASE_DIR/$name" "$PREVIOUS_STAGE/$name"
      fi
    done
    previous_name="previous.${PREVIOUS_STAGE##*.}"
    PREVIOUS_DIR="$RELEASES_DIR/$previous_name"
    mv "$PREVIOUS_STAGE" "$PREVIOUS_DIR"
    initial_relative="${PREVIOUS_DIR#"$INSTALL_DIR/"}"
  fi
  initial_link="$INSTALL_DIR/.opencoding-source-current.initial.$$"
  ln -s "$initial_relative" "$initial_link"
  python3 -c 'import os, sys; os.replace(sys.argv[1], sys.argv[2])' \
    "$initial_link" "$CURRENT_LINK"
fi

for name in opencoding-daemon opencoding-cli opencoding; do
  desired=".opencoding-source-current/$name"
  if [ -L "$INSTALL_DIR/$name" ] &&
     [ "$(readlink "$INSTALL_DIR/$name")" = "$desired" ]; then
    continue
  fi
  temporary="$INSTALL_DIR/.$name.link.$$"
  rm -f "$temporary"
  ln -s "$desired" "$temporary"
  python3 -c 'import os, sys; os.replace(sys.argv[1], sys.argv[2])' \
    "$temporary" "$INSTALL_DIR/$name"
done
test_checkpoint after-public-links

next_link="$INSTALL_DIR/.opencoding-source-current.next.$$"
ln -s "$release_relative" "$next_link"
# BSD mv follows a destination symlink to a directory, which would place the
# temporary link inside the old release instead of switching the public suite.
# os.replace operates on the symlink itself and is atomic on one filesystem.
python3 -c 'import os, sys; os.replace(sys.argv[1], sys.argv[2])' \
  "$next_link" "$CURRENT_LINK"
test_checkpoint after-current-switch

"$INSTALL_DIR/opencoding-daemon" --self-test >/dev/null
"$INSTALL_DIR/opencoding-cli" --self-test >/dev/null
"$INSTALL_DIR/opencoding" --help >/dev/null
RELEASE_DIR=""
trap - EXIT HUP INT TERM

echo "Installed Opencoding Community from source to $INSTALL_DIR"
echo "A running Opencoding service is not restarted automatically; run 'opencoding restart' before using the upgraded installation."
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "Add $INSTALL_DIR to PATH before running opencoding." ;;
esac
