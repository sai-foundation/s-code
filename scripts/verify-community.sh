#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
command -v python3 >/dev/null 2>&1 || { echo "python3 is required" >&2; exit 1; }
cd "$ROOT"
python3 scripts/check-community-tree.py
python3 scripts/check-community-secrets.py

mkdir -p "$ROOT/.work"
TASK="$(mktemp -d "$ROOT/.work/verify-community.XXXXXX")"
cleanup() {
  status=$?
  if [ "$status" -eq 0 ]; then
    find "$TASK" -depth -delete
  else
    echo "Community verification failed; retained $TASK" >&2
  fi
  exit "$status"
}
trap cleanup EXIT HUP INT TERM

export TMPDIR="$TASK/tmp"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$TASK/target}"
mkdir -p "$TMPDIR"

command -v cargo >/dev/null 2>&1 || { echo "cargo is required" >&2; exit 1; }
command -v npm >/dev/null 2>&1 || { echo "npm is required" >&2; exit 1; }
cargo deny --version >/dev/null 2>&1 || { echo "cargo-deny is required" >&2; exit 1; }
cargo audit --version >/dev/null 2>&1 || { echo "cargo-audit is required" >&2; exit 1; }
cargo about --version >/dev/null 2>&1 || { echo "cargo-about is required" >&2; exit 1; }

python3 scripts/check-doc-links.py README.md docs clients compliance
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo deny check
cargo about generate about.hbs > "$TASK/THIRD_PARTY_LICENSES.html"
test -s "$TASK/THIRD_PARTY_LICENSES.html"
grep -q '>serde 1\.' "$TASK/THIRD_PARTY_LICENSES.html"
grep -q 'MIT License' "$TASK/THIRD_PARTY_LICENSES.html"
tests/test-supply-chain.sh
tests/test-protocol-bindings.sh
npm ci --prefix "$ROOT/web"
npm audit --prefix "$ROOT/web" --audit-level=high
tests/test-web-build.sh
tests/test-install.sh
tests/test-update.sh

echo "Community source verification passed"
