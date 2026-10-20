#!/bin/sh
# Build the pinned, flat x86_64-glibc runtime bundle consumed by Harbor.
set -eu

usage() {
  cat <<'EOF'
Usage: integrations/harbor/build-bundle.sh

Build a normalized S-Code runtime archive for x86_64 Linux with glibc.
The Git worktree must be clean and all pinned build tools must already exist.
The daemon is built with the Harbor-only harbor-container-env Cargo feature.

Environment:
  S_CODE_HARBOR_OUTPUT_DIR  Output directory (default: .work/harbor)
  CARGO_TARGET_DIR          Cargo build directory
                            (default: .work/harbor/cargo-target)

Outputs:
  s-code-<commit>-x86_64-unknown-linux-gnu.tar.gz
  s-code-<commit>-x86_64-unknown-linux-gnu.tar.gz.sha256
EOF
}

if [ "$#" -eq 1 ]; then
  case "$1" in
    --help|-h) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
fi
[ "$#" -eq 0 ] || { usage >&2; exit 2; }

fail() {
  echo "build-bundle: $*" >&2
  exit 1
}

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd -P)"
[ "$(git -C "$ROOT" rev-parse --show-toplevel 2>/dev/null)" = "$ROOT" ] ||
  fail "the script must run from its S-Code Git checkout"

[ "$(uname -s)" = Linux ] || fail "bundles can only be built on Linux"
[ "$(uname -m)" = x86_64 ] || fail "bundles require the x86_64 architecture"
TARGET=x86_64-unknown-linux-gnu
# Only this bundle enables the container environment mode for agent commands;
# the manifest records the features so the adapter can refuse other builds.
CARGO_FEATURES=s-code-daemon/harbor-container-env

for required_command in \
  cargo cmp cut find getconf git install mkdir mktemp mv node npm python3 rustc sed uname
do
  command -v "$required_command" >/dev/null 2>&1 ||
    fail "missing required command: $required_command"
done

GLIBC_VERSION="$(getconf GNU_LIBC_VERSION 2>/dev/null || true)"
python3 - "$GLIBC_VERSION" <<'PY' || fail "a detectable glibc runtime is required"
import re
import sys

raise SystemExit(re.fullmatch(r"glibc [0-9]+(?:\.[0-9]+)+", sys.argv[1]) is None)
PY

PYTHON_VERSION="$(python3 -c 'import platform; print(platform.python_version())')"
python3 -c 'import sys; raise SystemExit(sys.version_info < (3, 9))' ||
  fail "Python 3.9 or newer is required"

PINNED_RUST="$(sed -n 's/^channel = "\([^"]*\)"/\1/p' "$ROOT/rust-toolchain.toml")"
[ -n "$PINNED_RUST" ] || fail "rust-toolchain.toml does not pin a Rust version"
RUSTC_VERSION="$(rustc --version)"
case "$RUSTC_VERSION" in
  "rustc $PINNED_RUST "*) ;;
  *) fail "rustc $PINNED_RUST is required (found: $RUSTC_VERSION)" ;;
esac
CARGO_VERSION="$(cargo --version)"
RUST_HOST="$(rustc -vV | sed -n 's/^host: //p')"
[ "$RUST_HOST" = "$TARGET" ] ||
  fail "the Rust host must be $TARGET (found: ${RUST_HOST:-unknown})"

PINNED_NODE="$(sed -n \
  's/^[[:space:]]*SOURCE_NODE_VERSION=\([0-9][0-9.]*\)$/\1/p' \
  "$ROOT/scripts/source-dependencies.sh")"
[ -n "$PINNED_NODE" ] ||
  fail "scripts/source-dependencies.sh does not pin a Node.js version"
NODE_VERSION="$(node --version)"
[ "$NODE_VERSION" = "v$PINNED_NODE" ] ||
  fail "Node.js $PINNED_NODE is required (found: $NODE_VERSION)"
NPM_VERSION="$(npm --version)"
ZLIB_VERSION="$(python3 -c 'import zlib; print(zlib.ZLIB_VERSION)')"

DIRTY="$(git -C "$ROOT" status --porcelain=v1 --untracked-files=all)"
[ -z "$DIRTY" ] || fail "Git worktree must be clean before building"
COMMIT="$(git -C "$ROOT" rev-parse --verify HEAD)"
TREE="$(git -C "$ROOT" rev-parse --verify 'HEAD^{tree}')"
SHORT_COMMIT="$(printf '%s' "$COMMIT" | cut -c 1-12)"

OUTPUT_DIR="${S_CODE_HARBOR_OUTPUT_DIR:-$ROOT/.work/harbor}"
case "$OUTPUT_DIR" in
  /*) ;;
  *) OUTPUT_DIR="$ROOT/$OUTPUT_DIR" ;;
esac
case "$OUTPUT_DIR" in
  /|"$ROOT") fail "output directory must be a dedicated directory" ;;
esac
if [ -n "${HOME:-}" ] && [ "$OUTPUT_DIR" = "$HOME" ]; then
  fail "output directory must not be the home directory"
fi
[ ! -L "$OUTPUT_DIR" ] || fail "output directory must not be a symlink"
mkdir -p "$OUTPUT_DIR"
OUTPUT_DIR="$(CDPATH= cd -- "$OUTPUT_DIR" && pwd -P)"

CARGO_BUILD_DIR="${CARGO_TARGET_DIR:-$OUTPUT_DIR/cargo-target}"
case "$CARGO_BUILD_DIR" in
  /*) ;;
  *) CARGO_BUILD_DIR="$ROOT/$CARGO_BUILD_DIR" ;;
esac
case "$CARGO_BUILD_DIR" in
  /|"$ROOT") fail "Cargo target must be a dedicated build directory" ;;
esac
if [ -n "${HOME:-}" ] && [ "$CARGO_BUILD_DIR" = "$HOME" ]; then
  fail "Cargo target must not be the home directory"
fi
export CARGO_TARGET_DIR="$CARGO_BUILD_DIR"

STAGE_DIR="$(mktemp -d "$OUTPUT_DIR/.bundle-stage.XXXXXX")"
ARCHIVE_TMP=""
SIDECAR_TMP=""
cleanup() {
  cleanup_status=$?
  trap - EXIT HUP INT TERM
  if [ -n "$STAGE_DIR" ] && [ -d "$STAGE_DIR" ]; then
    find "$STAGE_DIR" -depth -delete 2>/dev/null || true
  fi
  for temporary in "$ARCHIVE_TMP" "$SIDECAR_TMP"; do
    [ -n "$temporary" ] && [ -f "$temporary" ] &&
      find "$temporary" -type f -delete 2>/dev/null || true
  done
  exit "$cleanup_status"
}
trap cleanup EXIT HUP INT TERM

export LC_ALL=C TZ=UTC SOURCE_DATE_EPOCH=0
# Do not let caller-specific paths leak into Rust payloads.  Unit separator is
# Cargo's documented encoded-rustflags delimiter and safely handles spaces.
RUSTFLAG_SEPARATOR="$(printf '\037')"
export CARGO_ENCODED_RUSTFLAGS="--remap-path-prefix=$ROOT=/usr/src/s-code${RUSTFLAG_SEPARATOR}\
--remap-path-prefix=${HOME:-/nonexistent}=/home/s-code"
unset RUSTFLAGS

echo "Building web bundle with Node.js $PINNED_NODE..."
npm ci --prefix "$ROOT/web" --no-audit --no-fund
npm run build --prefix "$ROOT/web"
git -C "$ROOT" diff --quiet -- web/app.js ||
  fail "web/app.js was stale; commit the regenerated bundle before building"

echo "Building S-Code runtime with Rust $PINNED_RUST..."
cargo build --locked --release --manifest-path "$ROOT/Cargo.toml" \
  -p s-code-daemon -p s-code-cli --features "$CARGO_FEATURES"

for runtime_file in s-code-daemon s-code-cli s-code-pdf-worker; do
  source_file="$CARGO_TARGET_DIR/release/$runtime_file"
  [ -x "$source_file" ] || fail "build did not produce $runtime_file"
  install -m 0755 "$source_file" "$STAGE_DIR/$runtime_file"
done
install -m 0755 "$ROOT/scripts/s-code" "$STAGE_DIR/s-code"

"$STAGE_DIR/s-code-daemon" --self-test >/dev/null
"$STAGE_DIR/s-code-cli" --self-test >/dev/null
"$STAGE_DIR/s-code-pdf-worker" --self-test >/dev/null
"$STAGE_DIR/s-code" --help >/dev/null

python3 - \
  "$STAGE_DIR" "$COMMIT" "$TREE" "$TARGET" \
  "$ROOT/Cargo.lock" "$ROOT/web/package-lock.json" \
  "$RUSTC_VERSION" "$CARGO_VERSION" "$NODE_VERSION" "$NPM_VERSION" \
  "$PYTHON_VERSION" "$ZLIB_VERSION" "$GLIBC_VERSION" "$CARGO_FEATURES" <<'PY'
import hashlib
import json
import os
from pathlib import Path
import stat
import sys

(
    stage_arg, commit, tree, target, cargo_lock_arg, web_lock_arg,
    rustc_version, cargo_version, node_version, npm_version,
    python_version, zlib_version, glibc_version, cargo_features,
) = sys.argv[1:]
stage = Path(stage_arg)

def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()

runtime_names = (
    "s-code",
    "s-code-cli",
    "s-code-daemon",
    "s-code-pdf-worker",
)
files = {}
for name in runtime_names:
    path = stage / name
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or stat.S_IMODE(metadata.st_mode) != 0o755:
        raise SystemExit(f"invalid staged runtime file: {name}")
    files[name] = {
        "mode": "0755",
        "sha256": digest(path),
        "size": metadata.st_size,
    }

manifest = {
    "build": {
        "cargo": cargo_version,
        "cargo_features": sorted(cargo_features.split(",")),
        "glibc": glibc_version,
        "node": node_version,
        "npm": npm_version,
        "python": python_version,
        "rustc": rustc_version,
        "zlib": zlib_version,
    },
    "files": files,
    "format": "s-code-harbor-bundle",
    "schema_version": 1,
    "source": {
        "commit": commit,
        "dirty": False,
        "locks": {
            "Cargo.lock": digest(Path(cargo_lock_arg)),
            "web/package-lock.json": digest(Path(web_lock_arg)),
        },
        "tree": tree,
    },
    "target": target,
}
(stage / "manifest.json").write_text(
    json.dumps(manifest, ensure_ascii=True, separators=(",", ":"), sort_keys=True) + "\n",
    encoding="utf-8",
)
os.chmod(stage / "manifest.json", 0o644)
PY

ARCHIVE_NAME="s-code-$SHORT_COMMIT-$TARGET.tar.gz"
ARCHIVE_PATH="$OUTPUT_DIR/$ARCHIVE_NAME"
ARCHIVE_TMP="$OUTPUT_DIR/.$ARCHIVE_NAME.tmp.$$"
[ ! -e "$ARCHIVE_TMP" ] && [ ! -L "$ARCHIVE_TMP" ] ||
  fail "temporary archive path already exists"

python3 - "$STAGE_DIR" "$ARCHIVE_TMP" <<'PY'
import gzip
from pathlib import Path
import tarfile
import sys

stage = Path(sys.argv[1])
output = Path(sys.argv[2])
names = (
    "manifest.json",
    "s-code",
    "s-code-cli",
    "s-code-daemon",
    "s-code-pdf-worker",
)

with output.open("xb") as raw_output:
    with gzip.GzipFile(
        filename="", mode="wb", compresslevel=9, fileobj=raw_output, mtime=0
    ) as compressed:
        with tarfile.open(fileobj=compressed, mode="w|", format=tarfile.USTAR_FORMAT) as archive:
            for name in names:
                path = stage / name
                metadata = path.stat()
                member = tarfile.TarInfo(name)
                member.size = metadata.st_size
                member.mode = 0o644 if name == "manifest.json" else 0o755
                member.mtime = 0
                member.uid = 0
                member.gid = 0
                member.uname = ""
                member.gname = ""
                with path.open("rb") as source:
                    archive.addfile(member, source)
PY
chmod 0644 "$ARCHIVE_TMP"

ARCHIVE_SHA256="$(python3 - "$ARCHIVE_TMP" <<'PY'
import hashlib
from pathlib import Path
import sys

digest = hashlib.sha256()
with Path(sys.argv[1]).open("rb") as stream:
    for block in iter(lambda: stream.read(1024 * 1024), b""):
        digest.update(block)
print(digest.hexdigest())
PY
)"

PYTHONDONTWRITEBYTECODE=1 python3 "$ROOT/integrations/harbor/runtime.py" \
  "$ARCHIVE_TMP" "$ARCHIVE_SHA256" >/dev/null

if [ -e "$ARCHIVE_PATH" ] || [ -L "$ARCHIVE_PATH" ]; then
  [ ! -L "$ARCHIVE_PATH" ] || fail "refusing existing archive symlink: $ARCHIVE_PATH"
  EXISTING_SHA256="$(python3 - "$ARCHIVE_PATH" <<'PY'
import hashlib
from pathlib import Path
import sys

digest = hashlib.sha256()
with Path(sys.argv[1]).open("rb") as stream:
    for block in iter(lambda: stream.read(1024 * 1024), b""):
        digest.update(block)
print(digest.hexdigest())
PY
)"
  [ "$EXISTING_SHA256" = "$ARCHIVE_SHA256" ] ||
    fail "existing archive differs for the same source commit: $ARCHIVE_PATH"
  find "$ARCHIVE_TMP" -type f -delete
  ARCHIVE_TMP=""
else
  mv "$ARCHIVE_TMP" "$ARCHIVE_PATH"
  ARCHIVE_TMP=""
fi

SIDECAR_PATH="$ARCHIVE_PATH.sha256"
SIDECAR_TMP="$OUTPUT_DIR/.$ARCHIVE_NAME.sha256.tmp.$$"
[ ! -e "$SIDECAR_TMP" ] && [ ! -L "$SIDECAR_TMP" ] ||
  fail "temporary sidecar path already exists"
printf '%s  %s\n' "$ARCHIVE_SHA256" "$ARCHIVE_NAME" > "$SIDECAR_TMP"
chmod 0644 "$SIDECAR_TMP"
if [ -e "$SIDECAR_PATH" ] || [ -L "$SIDECAR_PATH" ]; then
  [ ! -L "$SIDECAR_PATH" ] || fail "refusing existing sidecar symlink: $SIDECAR_PATH"
  cmp -s "$SIDECAR_TMP" "$SIDECAR_PATH" ||
    fail "existing digest sidecar differs: $SIDECAR_PATH"
  find "$SIDECAR_TMP" -type f -delete
  SIDECAR_TMP=""
else
  mv "$SIDECAR_TMP" "$SIDECAR_PATH"
  SIDECAR_TMP=""
fi

# The selected output directory need not be Git-ignored, but tracked sources
# must remain byte-for-byte identical to the tree recorded in the manifest.
git -C "$ROOT" diff --quiet && git -C "$ROOT" diff --cached --quiet ||
  fail "build modified the tracked Git worktree"

echo "Bundle: $ARCHIVE_PATH"
echo "SHA-256: $ARCHIVE_SHA256"
