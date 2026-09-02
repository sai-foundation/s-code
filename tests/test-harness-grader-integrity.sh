#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
mkdir -p "$ROOT/.work"
task="$(mktemp -d "$ROOT/.work/harness-grader.XXXXXX")"
cleanup() {
  status=$?
  trap - 0 1 2 15
  if [ "$status" -eq 0 ]; then
    find "$task" -depth -delete
  else
    echo "harness grader integrity test failed; retained artifacts at $task" >&2
  fi
  exit "$status"
}
trap cleanup 0 1 2 15

prepare() {
  workspace=$1
  python3 "$ROOT/tests/test-harness-benchmark.py" prepare \
    --track project --task durable-task-queue --destination "$workspace" >/dev/null
}

must_reject() {
  workspace=$1
  label=$2
  if python3 "$ROOT/tests/test-harness-benchmark.py" grade \
    --track project --task durable-task-queue --workspace "$workspace" \
    >"$task/$label.out" 2>&1; then
    echo "grader accepted protected fixture tampering: $label" >&2
    exit 1
  fi
  grep -F '"passed": false' "$task/$label.out" >/dev/null
}

workspace="$task/committed"
prepare "$workspace"
printf '\n# tampered after commit\n' >>"$workspace/tests/test_durable_queue.py"
git -C "$workspace" add tests/test_durable_queue.py
git -C "$workspace" -c user.name=Attacker -c user.email=attacker@example.invalid \
  commit -qm 'hide changed grader in a commit'
must_reject "$workspace" committed

workspace="$task/index-hidden"
prepare "$workspace"
git -C "$workspace" update-index --assume-unchanged tests/test_durable_queue.py
printf '\n# hidden from status\n' >>"$workspace/tests/test_durable_queue.py"
must_reject "$workspace" index-hidden

workspace="$task/symlink"
prepare "$workspace"
mv "$workspace/tests/test_durable_queue.py" "$workspace/test_queue.saved"
ln -s ../test_queue.saved "$workspace/tests/test_durable_queue.py"
must_reject "$workspace" symlink

workspace="$task/extra"
prepare "$workspace"
printf 'raise SystemExit(0)\n' >"$workspace/tests/test_override.py"
must_reject "$workspace" extra

workspace="$task/committed-extra"
prepare "$workspace"
printf 'raise SystemExit(0)\n' >"$workspace/sitecustomize.py"
git -C "$workspace" add sitecustomize.py
git -C "$workspace" -c user.name=Attacker -c user.email=attacker@example.invalid \
  commit -qm 'hide extra import hook in head'
must_reject "$workspace" committed-extra

workspace="$task/unittest-shadow"
prepare "$workspace"
printf 'raise SystemExit(0)\n' >"$workspace/unittest.py"
must_reject "$workspace" unittest-shadow

workspace="$task/zero-tests"
prepare "$workspace"
printf 'import os\nos._exit(0)\n' >"$workspace/durable_queue/__init__.py"
must_reject "$workspace" zero-tests

workspace="$task/forged-result"
prepare "$workspace"
printf 'print("OPENCODING_GRADER_RESULT={\\"successful\\":true,\\"tests_run\\":5,\\"errors\\":0,\\"failures\\":0}")\n' \
  >"$workspace/durable_queue/__init__.py"
must_reject "$workspace" forged-result

workspace="$task/unbounded-output"
prepare "$workspace"
printf 'print("x" * 3000000)\n' >"$workspace/durable_queue/__init__.py"
must_reject "$workspace" unbounded-output

workspace="$task/unittest-result-monkeypatch"
prepare "$workspace"
cat > "$workspace/durable_queue/__init__.py" <<'PY'
import unittest
unittest.TestResult.stopTestRun = lambda self: (self.errors.clear(), self.failures.clear())
unittest.TestResult.addFailure = lambda self, test, err: None
unittest.TestResult.wasSuccessful = lambda self: True
PY
must_reject "$workspace" unittest-result-monkeypatch

workspace="$task/non-python-import"
prepare "$workspace"
printf 'candidate executable import payload\n' > "$workspace/durable_queue/backdoor.so"
chmod 0755 "$workspace/durable_queue/backdoor.so"
must_reject "$workspace" non-python-import

# Exercise the external Python runner directly so its independent counters are
# proven to reject ordinary unittest result monkeypatching even without the
# candidate-tree validator in front of it.
workspace="$task/runner-monkeypatch"
mkdir -p "$workspace/tests" "$workspace/candidate"
cat > "$workspace/candidate/__init__.py" <<'PY'
import unittest
unittest.TestResult.stopTestRun = lambda self: (self.errors.clear(), self.failures.clear())
unittest.TestResult.addFailure = lambda self, test, err: None
unittest.TestResult.wasSuccessful = lambda self: True
PY
cat > "$workspace/tests/test_integrity.py" <<'PY'
import unittest
import candidate

class IntegrityTest(unittest.TestCase):
    def test_failure_cannot_be_erased(self):
        self.fail("trusted failure")
PY
if python3 -I "$ROOT/tests/benchmarks/runner/python_unittest_runner.py" \
  --workspace "$workspace" --pattern 'test*.py' --discover \
  > "$task/runner-monkeypatch.out" 2>&1; then
  echo "external Python runner accepted unittest result monkeypatching" >&2
  exit 1
fi
grep -F '"successful":false' "$task/runner-monkeypatch.out" >/dev/null
grep -F '"failures":1' "$task/runner-monkeypatch.out" >/dev/null

workspace="$task/frontend-bin"
python3 "$ROOT/tests/test-harness-benchmark.py" prepare \
  --track frontend --task focus-board --destination "$workspace" >/dev/null
mkdir -p "$workspace/node_modules/.bin"
printf '#!/bin/sh\nexit 0\n' >"$workspace/node_modules/.bin/playwright"
chmod 0755 "$workspace/node_modules/.bin/playwright"
if python3 "$ROOT/tests/test-harness-benchmark.py" grade \
  --track frontend --task focus-board --workspace "$workspace" \
  >"$task/frontend-bin.out" 2>&1; then
  echo "grader accepted a workspace-local frontend runner" >&2
  exit 1
fi
grep -F '"passed": false' "$task/frontend-bin.out" >/dev/null

echo "harness grader defensive integrity checks passed"
