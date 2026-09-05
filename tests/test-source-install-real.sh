#!/bin/sh
# Build and exercise the actual source-installed release, in isolated state.
set -eu
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
mkdir -p "$ROOT/.work"
TASK="$(mktemp -d "$ROOT/.work/source-install-real.XXXXXX")"
cleanup() {
  status=$?
  trap - EXIT HUP INT TERM
  if [ "$status" -eq 0 ]; then
    find "$TASK" -depth -delete
  else
    echo "real source installation failed; retained $TASK" >&2
  fi
  exit "$status"
}
trap cleanup EXIT HUP INT TERM
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$TASK/target}"
OPENCODING_INSTALL_DIR="$TASK/bin" "$ROOT/scripts/install-from-source.sh"
OPENCODING_TEST_BIN_DIR="$TASK/bin" "$ROOT/tests/test-first-run.sh"
echo "real source installation and installed-release first run passed"
