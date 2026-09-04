#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
SCOPE="${1:-all}"
case "$SCOPE" in
  all|policy|rust|web|runtime) ;;
  *)
    echo "usage: scripts/verify-community.sh [all|policy|rust|web|runtime]" >&2
    exit 2
    ;;
esac
command -v python3 >/dev/null 2>&1 || { echo "python3 is required" >&2; exit 1; }
cd "$ROOT"

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
export OPENCODING_SHARED_TEST_TMPDIR="$TMPDIR"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$TASK/target}"
mkdir -p "$TMPDIR"
chmod 0700 "$TMPDIR"

require_cargo() {
  command -v cargo >/dev/null 2>&1 || { echo "cargo is required" >&2; exit 1; }
}

require_npm() {
  command -v npm >/dev/null 2>&1 || { echo "npm is required" >&2; exit 1; }
}

verify_policy_preflight() {
  python3 scripts/check-community-tree.py
  python3 scripts/check-community-secrets.py --history
  python3 scripts/check-doc-links.py README.md docs clients compliance
}

verify_rust() {
  require_cargo
  cargo fmt --all -- --check
  cargo clippy --locked --workspace --all-targets -- -D warnings
  cargo test --locked --workspace
}

verify_policy() {
  require_cargo
  require_npm
  cargo deny --version >/dev/null 2>&1 || { echo "cargo-deny is required" >&2; exit 1; }
  cargo audit --version >/dev/null 2>&1 || { echo "cargo-audit is required" >&2; exit 1; }
  cargo about --version >/dev/null 2>&1 || { echo "cargo-about is required" >&2; exit 1; }
  cargo deny check
  cargo about generate about.hbs > "$TASK/THIRD_PARTY_LICENSES.html"
  test -s "$TASK/THIRD_PARTY_LICENSES.html"
  grep -q '>serde 1\.' "$TASK/THIRD_PARTY_LICENSES.html"
  grep -q 'MIT License' "$TASK/THIRD_PARTY_LICENSES.html"
  tests/test-supply-chain.sh
  tests/test-protocol-bindings.sh
  tests/test-dco.sh
  tests/test-community-candidate.sh
  tests/test-ci-workflow.sh
  python3 tests/test-harness-benchmark.py validate
  tests/test-harness-grader-integrity.sh
  npm ci --prefix "$ROOT/tests/benchmarks/runner" --no-audit --no-fund
  npm audit --prefix "$ROOT/tests/benchmarks/runner" --audit-level=high
}

verify_web() {
  require_npm
  npm ci --prefix "$ROOT/web" --no-audit --no-fund
  npm audit --prefix "$ROOT/web" --audit-level=high
  tests/test-web-build.sh
  tests/test-community-docs-site.sh
  tests/test-source-install.sh
}

verify_runtime() {
  require_cargo
  tests/test-first-run.sh
  tests/test-privacy-security-use-cases.sh
}

case "$SCOPE" in
  all)
    verify_policy_preflight
    verify_rust
    verify_policy
    verify_web
    verify_runtime
    echo "Community source verification passed"
    ;;
  policy)
    verify_policy_preflight
    verify_policy
    echo "Community policy verification passed"
    ;;
  rust)
    verify_rust
    echo "Community Rust verification passed"
    ;;
  web)
    verify_web
    echo "Community Web and documentation verification passed"
    ;;
  runtime)
    verify_runtime
    echo "Community runtime verification passed"
    ;;
esac
