#!/bin/sh
set -eu
ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
python3 "$ROOT/tests/test-ci-plan.py"
echo "Affected-area CI routing, fail-closed gate and full release workflow checks passed"
