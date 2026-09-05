#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
mkdir -p "$ROOT/.work"
task="$(mktemp -d "$ROOT/.work/community-candidate.XXXXXX")"
trap 'find "$task" -depth -delete' EXIT HUP INT TERM

repository="$task/repository"
mkdir -p "$repository"
git -C "$repository" init --initial-branch=main >/dev/null
printf 'qualified tree\n' >"$repository/source.txt"
git -C "$repository" add source.txt
git -C "$repository" -c user.name='Candidate Test' \
  -c user.email='candidate@example.invalid' commit -m 'qualified tree' >/dev/null

qualified_revision="$(git -C "$repository" rev-parse HEAD)"
head_revision="fedcba9876543210fedcba9876543210fedcba98"
python3 "$ROOT/scripts/community-candidate.py" qualify \
  --root "$repository" \
  --output "$task/qualification.json" \
  --repository example/community \
  --tested-revision "$qualified_revision" \
  --pull-request 42 \
  --pull-request-head "$head_revision" \
  --workflow-run-id 1001 \
  --workflow-run-attempt 1 \
  --profile full \
  --check source-checks --check dco --check linux --check macos --check windows \
  --check web --check docs --check vscode --check jetbrains --check benchmarks

git -C "$repository" -c user.name='Candidate Test' \
  -c user.email='candidate@example.invalid' commit --allow-empty -m 'merge fixture' >/dev/null
candidate_revision="$(git -C "$repository" rev-parse HEAD)"
python3 "$ROOT/scripts/community-candidate.py" finalize \
  --root "$repository" \
  --qualification "$task/qualification.json" \
  --output "$task/candidate.json" \
  --repository example/community \
  --candidate-revision "$candidate_revision" \
  --pull-request 42 \
  --expected-head-revision "$head_revision"

python3 - "$task/qualification.json" "$task/candidate.json" <<'PY'
import hashlib
import json
import pathlib
import sys

qualification_path = pathlib.Path(sys.argv[1])
qualification = json.loads(qualification_path.read_text())
candidate = json.loads(pathlib.Path(sys.argv[2]).read_text())
assert qualification["outcome"] == "passed"
assert candidate["outcome"] == "release_ready"
assert qualification["schema_version"] == 3
assert candidate["schema_version"] == 3
assert candidate["candidate_tree"] == qualification["tested_tree"]
assert candidate["qualification_run_id"] == 1001
assert candidate["qualification_evidence_sha256"] == hashlib.sha256(
    qualification_path.read_bytes()
).hexdigest()
PY

# Selective/legacy artifacts must never be promoted to release-ready, even if
# the tree and every other identity field match a real full run.
python3 - "$ROOT/scripts/community-candidate.py" "$repository" "$task" "$candidate_revision" "$head_revision" <<'PY'
import json
from pathlib import Path
import subprocess
import sys

script, root, task, revision, head = sys.argv[1:]
task = Path(task)
qualification = json.loads((task / "qualification.json").read_text())
candidate = json.loads((task / "candidate.json").read_text())
for field, value in (("passed_checks", ["source-checks"]), ("schema_version", 2), ("profile", "selective")):
    bad = dict(qualification, **{field: value})
    (task / "bad-qualification.json").write_text(json.dumps(bad))
    result = subprocess.run(["python3", script, "finalize", "--root", root,
        "--qualification", str(task / "bad-qualification.json"), "--output", str(task / "bad-candidate.json"),
        "--repository", "example/community", "--candidate-revision", revision,
        "--pull-request", "42", "--expected-head-revision", head], capture_output=True, text=True)
    assert result.returncode != 0, (field, value)
    bad = dict(candidate, **{field: value})
    (task / "bad-candidate.json").write_text(json.dumps(bad))
    result = subprocess.run(["python3", script, "verify", "--root", root,
        "--candidate", str(task / "bad-candidate.json"), "--repository", "example/community",
        "--expected-revision", revision, "--workflow-run-id", "1001", "--workflow-run-attempt", "1"], capture_output=True, text=True)
    assert result.returncode != 0, (field, value)

verify = ["python3", script, "verify", "--root", root, "--candidate", str(task / "candidate.json"),
          "--repository", "example/community", "--expected-revision", revision, "--workflow-run-attempt", "1"]
wrong_run = subprocess.run(verify + ["--workflow-run-id", "1002"], capture_output=True, text=True)
assert wrong_run.returncode != 0 and "selected full workflow run" in wrong_run.stderr
wrong_attempt = subprocess.run(verify + ["--workflow-run-id", "1001", "--workflow-run-attempt", "2"], capture_output=True, text=True)
assert wrong_attempt.returncode != 0 and "selected workflow attempt" in wrong_attempt.stderr
missing_codeql = subprocess.run(verify + ["--workflow-run-id", "1001", "--require-codeql"], capture_output=True, text=True)
assert missing_codeql.returncode != 0 and "including CodeQL" in missing_codeql.stderr
candidate["passed_checks"] = sorted(candidate["passed_checks"] + ["codeql"])
(task / "candidate.json").write_text(json.dumps(candidate))
subprocess.run(verify + ["--workflow-run-id", "1001", "--require-codeql"], check=True)
PY

python3 "$ROOT/scripts/community-candidate.py" verify \
  --root "$repository" \
  --candidate "$task/candidate.json" \
  --repository example/community \
  --expected-revision "$candidate_revision" --workflow-run-id 1001 --workflow-run-attempt 1

if python3 "$ROOT/scripts/community-candidate.py" verify \
  --root "$repository" \
  --candidate "$task/candidate.json" \
  --repository example/community \
  --expected-revision '0000000000000000000000000000000000000000' --workflow-run-id 1001 --workflow-run-attempt 1 \
  >"$task/verify-mismatch.out" 2>&1; then
  echo "candidate evidence accepted the wrong release revision" >&2
  exit 1
fi
grep -F 'candidate evidence is not bound to the release revision' \
  "$task/verify-mismatch.out" >/dev/null

printf 'changed after qualification\n' >"$repository/source.txt"
git -C "$repository" add source.txt
git -C "$repository" -c user.name='Candidate Test' \
  -c user.email='candidate@example.invalid' commit -m 'unqualified change' >/dev/null
changed_revision="$(git -C "$repository" rev-parse HEAD)"
if python3 "$ROOT/scripts/community-candidate.py" finalize \
  --root "$repository" \
  --qualification "$task/qualification.json" \
  --output "$task/rejected.json" \
  --repository example/community \
  --candidate-revision "$changed_revision" \
  --pull-request 42 \
  --expected-head-revision "$head_revision" \
  >"$task/rejected.out" 2>"$task/rejected.err"; then
  echo "candidate evidence accepted an unqualified tree" >&2
  exit 1
fi
grep -F 'candidate tree differs from the tree that passed qualification' \
  "$task/rejected.err" >/dev/null

echo "Community candidate evidence binding passed"
