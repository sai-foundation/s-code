#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
WORKFLOW="$ROOT/.github/workflows/ci.yml"
RELEASE_WORKFLOW="$ROOT/.github/workflows/release.yml"
mkdir -p "$ROOT/.work"
TASK="$(mktemp -d "$ROOT/.work/ci-workflow.XXXXXX")"
trap 'find "$TASK" -depth -delete' EXIT HUP INT TERM

for job in source-policy rust-linux web-docs source-gate cli-e2e rust-platforms; do
  grep -F "  $job:" "$WORKFLOW" >/dev/null || {
    echo "Community CI is missing the $job job" >&2
    exit 1
  }
done

grep -F 'needs: [source-policy, rust-linux, web-docs, cli-e2e]' "$WORKFLOW" >/dev/null
grep -F 'CARGO_TARGET_DIR: ${{ github.workspace }}/.work/ci-cache/target' "$WORKFLOW" >/dev/null
grep -F 'workspaces: . -> .work/ci-cache/target' "$WORKFLOW" >/dev/null
grep -F 'S_CODE_SHARED_TEST_TMPDIR: ${{ github.workspace }}/.work/ci-runtime/tmp' \
  "$WORKFLOW" >/dev/null
grep -F 'S_CODE_SHARED_NPM_CACHE: ${{ github.workspace }}/.work/npm-cache' \
  "$WORKFLOW" >/dev/null
grep -F 'S_CODE_SKIP_NETWORK_AUDIT: "1"' "$WORKFLOW" >/dev/null
if grep -F 'S_CODE_SKIP_NETWORK_AUDIT' "$RELEASE_WORKFLOW" >/dev/null; then
  echo "Community release verification must not skip network dependency audits" >&2
  exit 1
fi
grep -F 'scripts/verify-community.sh policy' "$WORKFLOW" >/dev/null
grep -F 'scripts/verify-community.sh rust' "$WORKFLOW" >/dev/null
grep -F 'scripts/verify-community.sh web' "$WORKFLOW" >/dev/null
test "$(grep -Fc 'Install Linux sandbox backend' "$WORKFLOW")" -eq 2
grep -F 'CARGO_TARGET_DIR: ${{ github.workspace }}/.work/ci-cache/target' \
  "$RELEASE_WORKFLOW" >/dev/null
grep -F 'tests/benchmarks/runner/package-lock.json' "$RELEASE_WORKFLOW" >/dev/null

if "$ROOT/scripts/verify-community.sh" unsupported-scope \
  >"$TASK/invalid.out" 2>&1; then
  echo "Community verifier accepted an unsupported CI scope" >&2
  exit 1
fi

echo "Community CI preserves the full source gate while running independent checks in parallel"
