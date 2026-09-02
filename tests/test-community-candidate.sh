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
  --check source-gate \
  --check rust-platforms \
  --check cli-e2e

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
assert qualification["schema_version"] == 2
assert candidate["schema_version"] == 2
assert candidate["candidate_tree"] == qualification["tested_tree"]
assert candidate["qualification_run_id"] == 1001
assert candidate["qualification_evidence_sha256"] == hashlib.sha256(
    qualification_path.read_bytes()
).hexdigest()
PY

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
