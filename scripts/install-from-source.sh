#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
SOURCE_DEPENDENCY_MODE=prompt
SOURCE_MODIFY_PATH=yes
for argument in "$@"; do
  case "$argument" in
    --yes|--check-deps|--no-install-deps)
      [ "$SOURCE_DEPENDENCY_MODE" = prompt ] || {
        echo 'Choose only one dependency option; see --help.' >&2; exit 2;
      }
      ;;
  esac
  case "$argument" in
    --yes) SOURCE_DEPENDENCY_MODE=yes ;;
    --check-deps) SOURCE_DEPENDENCY_MODE=check ;;
    --no-install-deps) SOURCE_DEPENDENCY_MODE=never ;;
    --no-modify-path) SOURCE_MODIFY_PATH=no ;;
    --help|-h)
      cat <<'HELP'
Usage: scripts/install-from-source.sh [--yes | --check-deps | --no-install-deps] [--no-modify-path]

Build and install S-Code. Existing dependencies are reused. If dependencies
are missing, interactive installs show a plan and ask before installing them.
  --yes              Install missing dependencies without the initial prompt.
                     System package installation may still need a sudo password.
  --check-deps       Only check prerequisites; do not install or build anything.
  --no-install-deps  Build using existing prerequisites; never bootstrap tools.
  --no-modify-path   Do not add the command path to shell configuration.

Rust and Node are build prerequisites, not application runtime requirements.
New managed tools live in ~/.cache/s-code/build-tools; existing Node is unchanged.
The installed command path is added to zsh, bash or fish configuration by default.
Linux requires Bubblewrap at runtime. Start from this checkout with ./s-code.
HELP
      exit 0
      ;;
    *) echo "Unknown installer argument: $argument" >&2; exit 2 ;;
  esac
done
INSTALL_DIR="${S_CODE_INSTALL_DIR:-$HOME/.local/bin}"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target/s-code-source}"

case "$INSTALL_DIR" in
  /|"$HOME"|"$ROOT")
    echo "install destination must be a dedicated binary directory" >&2
    exit 2
    ;;
  /*) ;;
  *) echo "install destination must be an absolute path" >&2; exit 2 ;;
esac
case "$INSTALL_DIR" in
  *:*|*'
'*) echo "install destination cannot contain colons or newlines" >&2; exit 2 ;;
esac
case "$TARGET_DIR" in
  /|"$HOME"|"$ROOT")
    echo "Cargo target must be a dedicated build directory" >&2
    exit 2
    ;;
esac

. "$ROOT/scripts/source-dependencies.sh"
prepare_source_dependencies
[ "$SOURCE_DEPENDENCY_MODE" != check ] || exit 0

command -v install >/dev/null 2>&1 || { echo "install is required" >&2; exit 1; }

mkdir -p "$INSTALL_DIR"
LOCK_FILE="$INSTALL_DIR/.s-code-source-install.lock"
if [ "${S_CODE_INSTALL_LOCK_HELD:-0}" != 1 ]; then
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
environment["S_CODE_INSTALL_LOCK_HELD"] = "1"
os.execve(script, [script, *arguments], environment)
' "$0" "$LOCK_FILE" "$@"
fi
RELEASES_DIR="$INSTALL_DIR/.s-code-source-releases"
CURRENT_LINK="$INSTALL_DIR/.s-code-source-current"
STAGE_DIR=""
RELEASE_DIR=""

test_checkpoint() {
  [ "${S_CODE_INSTALL_TEST_CHECKPOINT:-}" = "$1" ] || return 0
  [ -n "${S_CODE_INSTALL_TEST_MARKER:-}" ] || return 0
  printf '%s\n' "$1" > "$S_CODE_INSTALL_TEST_MARKER"
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
  -p s-code-daemon -p s-code-cli

STAGE_DIR="$(mktemp -d "$RELEASES_DIR/.stage.XXXXXX")"

for binary in s-code-daemon s-code-cli; do
  source_file="$TARGET_DIR/release/$binary"
  [ -x "$source_file" ] || { echo "build did not produce $binary" >&2; exit 1; }
  install -m 0755 "$source_file" "$STAGE_DIR/$binary"
done
for mapping in "s-code:scripts/s-code"; do
  destination="${mapping%%:*}"
  source_relative="${mapping#*:}"
  install -m 0755 "$ROOT/$source_relative" "$STAGE_DIR/$destination"
done

# Qualify the complete staged suite before touching an installed command.
"$STAGE_DIR/s-code-daemon" --self-test >/dev/null
"$STAGE_DIR/s-code-cli" --self-test >/dev/null
"$STAGE_DIR/s-code" --help >/dev/null

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
  for name in s-code-daemon s-code-cli s-code; do
    [ -e "$INSTALL_DIR/$name" ] || [ -L "$INSTALL_DIR/$name" ] || continue
    existing=$((existing + 1))
  done
  if [ "$existing" -gt 0 ]; then
    PREVIOUS_STAGE="$(mktemp -d "$RELEASES_DIR/.stage.previous.XXXXXX")"
    for name in s-code-daemon s-code-cli s-code; do
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
  initial_link="$INSTALL_DIR/.s-code-source-current.initial.$$"
  ln -s "$initial_relative" "$initial_link"
  python3 -c 'import os, sys; os.replace(sys.argv[1], sys.argv[2])' \
    "$initial_link" "$CURRENT_LINK"
fi

for name in s-code-daemon s-code-cli s-code; do
  desired=".s-code-source-current/$name"
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

next_link="$INSTALL_DIR/.s-code-source-current.next.$$"
ln -s "$release_relative" "$next_link"
# BSD mv follows a destination symlink to a directory, which would place the
# temporary link inside the old release instead of switching the public suite.
# os.replace operates on the symlink itself and is atomic on one filesystem.
python3 -c 'import os, sys; os.replace(sys.argv[1], sys.argv[2])' \
  "$next_link" "$CURRENT_LINK"
test_checkpoint after-current-switch

"$INSTALL_DIR/s-code-daemon" --self-test >/dev/null
"$INSTALL_DIR/s-code-cli" --self-test >/dev/null
"$INSTALL_DIR/s-code" --help >/dev/null
RELEASE_DIR=""
trap - EXIT HUP INT TERM

echo "Installed S-Code from source to $INSTALL_DIR"
if [ "$SOURCE_MODIFY_PATH" = no ]; then
  python3 "$ROOT/scripts/configure-shell-path.py" --install-dir "$INSTALL_DIR" --no-modify-path
else
  python3 "$ROOT/scripts/configure-shell-path.py" --install-dir "$INSTALL_DIR"
fi
echo "From this checkout you can also run: ./s-code"
