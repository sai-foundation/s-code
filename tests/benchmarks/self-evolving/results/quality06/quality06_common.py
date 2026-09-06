"""Private prospective quality06 bindings. No daemon, grader, or model invocation."""
from __future__ import annotations
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys

ROUND = "quality06"
FAMILIES = ("flow", "queue", "report")
ARMS = ("off", "raw", "learned")
SEEDS = (17, 29, 43)
HELPERS = ("quality06_common.py", "prepare-quality06-freeze.py", "run-quality06.py", "analyze-quality06.py", "test-quality06-helpers.py", "test-quality06-analysis.py")
BENCHMARK = "tests/benchmarks/self-evolving"
ARTIFACTS = ".work/quality06-freeze-artifacts"
FREEZE = ".work/quality06-development-freeze.json"
APPROVAL = ".work/quality06-freeze-independent-review.json"
PIPELINE = ".work/quality06-pipeline.json"
LAUNCH = ".work/quality06-launch.json"


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def read(path):
    return json.loads(Path(path).read_text())


def write_new(path, value):
    path = Path(path)
    with path.open("x", encoding="utf8") as stream:
        json.dump(value, stream, indent=2, allow_nan=False)
        stream.write("\n")
    path.chmod(0o600)


def digest_object(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def schedule():
    result = {}
    for family in FAMILIES:
        first = {"report": 0, "queue": 1, "flow": 2}[family]
        result[family] = []
        for index, seed in enumerate(SEEDS):
            offset = (first + index) % 3
            arms = ARMS[offset:] + ARMS[:offset]
            result[family].extend(dict(seed=seed, arm=arm, order_position=position) for position, arm in enumerate(arms))
    return result


def configuration():
    return dict(round=ROUND, phase="development-only", model="z-ai/glm-5.3", provider="z-ai/fp8", fallback=False,
                reasoning_effort="low", temperature=0, training_seed=17, seeds=list(SEEDS),
                family_order=list(FAMILIES), schedule=schedule(), family_cost_cap_usd=15, task_cost_cap_usd=1.5,
                global_admission_cap_usd=45, proxy_calls_cap_per_family=2000, agent_dispatch_cap_per_task=48,
                tool_call_cap_per_task=64, task_token_cap=1_000_000, task_outer_timeout_seconds=900,
                coding_initial_max_output_tokens=8192, coding_adaptive_max_output_tokens=32768,
                reflection_max_output_tokens=1024, planned_training_attempts=3, planned_development_attempts=27,
                development_screen=dict(horizon=12, minimum_token_reduction_vs_off=.20, no_observed_success_loss=True,
                                        known_grading_and_usage=True, known_dollar_costs=True, no_budget_denials=True, minimum_distilled_exposure_requests=1,
                                        independent_review_required=True, authorizes_holdout_reveal=False),
                planned_artifacts=dict(approval=APPROVAL, launch=LAUNCH, completion=PIPELINE,
                    analysis=".work/quality06-reviewed/quality06.json", independent_audit=".work/quality06-reviewed/independent-review.json"),
                selection="All 30 planned slots retained; failed training leaves its dependent development slots not_run; no retries, selective replacement, or pooling of historical rounds.")


def safe_relative(name):
    path = Path(name)
    if not isinstance(name, str) or path.is_absolute() or not path.parts or ".." in path.parts:
        raise ValueError("Expected a contained relative artifact path")
    return path


def regular_file(root, name):
    relative = safe_relative(name)
    for index in range(1, len(relative.parts) + 1):
        if root.joinpath(*relative.parts[:index]).is_symlink():
            raise ValueError("A bound artifact traverses a symlink")
    path = root / relative
    if not path.is_file():
        raise ValueError("A required bound artifact is missing")
    return path


def tree_hash(root):
    if not root.is_dir():
        raise ValueError("Required tree missing")
    digest = hashlib.sha256()
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root)
        if any(part in {".git", "__pycache__"} for part in relative.parts):
            continue
        if path.is_symlink():
            raise ValueError("Tree contains a symlink")
        if path.is_file():
            digest.update(str(relative).encode() + b"\0" + path.read_bytes() + b"\0")
    return digest.hexdigest()


def committed_inventory(root):
    """Bind HEAD blobs too, catching tracked edits hidden by assume-unchanged."""
    root = root.resolve()
    def git(*args):
        return subprocess.check_output(["git", "-C", str(root), *args])
    if git("status", "--porcelain=v1", "--untracked-files=all"):
        raise ValueError("Commit the candidate first: source/index must be clean")
    revision = git("rev-parse", "HEAD").decode().strip()
    algorithm = git("rev-parse", "--show-object-format").decode().strip()
    if algorithm not in {"sha1", "sha256"}:
        raise ValueError("Unsupported Git object format")
    files = {}
    for entry in git("ls-tree", "-r", "-z", "--full-tree", "HEAD").split(b"\0"):
        if not entry:
            continue
        metadata, encoded_name = entry.split(b"\t", 1)
        mode, kind, blob = metadata.decode().split()
        name = encoded_name.decode()
        if kind != "blob" or mode not in {"100644", "100755"}:
            raise ValueError("Source snapshots require regular tracked files")
        path = regular_file(root, name)
        data = path.read_bytes()
        if len(data) > 5_000_000:
            raise ValueError("Tracked file exceeds the harness snapshot bound")
        actual_blob = hashlib.new(algorithm, f"blob {len(data)}\0".encode() + data).hexdigest()
        if actual_blob != blob:
            raise ValueError("Working source differs from the committed blob")
        files[name] = hashlib.sha256(data).hexdigest()
    if not files:
        raise ValueError("Empty committed source inventory")
    return revision, files


def validate_configuration(freeze):
    for key, expected in configuration().items():
        if freeze.get(key) != expected:
            raise ValueError(f"Frozen configuration differs: {key}")


def public_safe(value):
    text = json.dumps(value, ensure_ascii=False)
    if any(term in text for term in ("/Users/", "/home/", "Bearer ", "sk-or-v1-", "BEGIN PRIVATE KEY")):
        raise ValueError("Historical report is not sanitized")


def history_inputs(manifest):
    rows = read(manifest)
    if not isinstance(rows, list) or not rows:
        raise ValueError("Historical manifest must contain separate sanitized reports")
    seen, kinds = set(), set()
    for row in rows:
        name = row.get("file")
        if not isinstance(name, str) or Path(name).name != name or not name.endswith(".json") or name in seen or name in {"quality06.json", "independent-review.json"}:
            raise ValueError("Historical report names must be unique JSON basenames")
        if row.get("kind") not in {"development", "confirmatory", "independent_review"}:
            raise ValueError("Only sanitized historical report artifacts are accepted")
        if row.get("round") == ROUND:
            raise ValueError("Current round cannot be included as historical evidence")
        if not isinstance(row.get("path"), str) or not row["path"] or not isinstance(row.get("sha256"), str):
            raise ValueError("Required historical evidence is pending an explicit path and digest")
        path = Path(row["path"])
        if not path.is_file() or path.is_symlink() or sha(path) != row.get("sha256"):
            raise ValueError("Historical report differs from its explicit binding")
        public_safe(read(path))
        seen.add(name)
        kinds.add((row.get("round"), row["kind"]))
    if not {("quality04", "development"), ("quality04", "confirmatory"),
            ("quality05", "development"), ("quality05", "independent_review")} <= kinds:
        raise ValueError("Retain quality04 phases and quality05 development/audit separately")
    return rows


def verify_freeze(root, freeze, *, require_clean=True):
    validate_configuration(freeze)
    if require_clean:
        revision, inventory = committed_inventory(root)
        if revision != freeze.get("revision") or digest_object(inventory) != freeze.get("source_sha256"):
            raise ValueError("Committed implementation differs from the freeze")
    frozen_inventory = read(regular_file(root, f"{ARTIFACTS}/source.json"))
    if digest_object(frozen_inventory) != freeze.get("source_sha256"):
        raise ValueError("Frozen source manifest differs")
    source_root = root / ARTIFACTS / "source"
    if any(path.is_symlink() for path in source_root.rglob("*")) or {str(path.relative_to(source_root)) for path in source_root.rglob("*") if path.is_file()} != set(frozen_inventory):
        raise ValueError("Frozen source inventory has unexpected entries")
    required_bindings = {f".work/{name}" for name in HELPERS} | {f"{ARTIFACTS}/private-helpers/{name}" for name in HELPERS} | {f"{ARTIFACTS}/s-code-daemon", f"{ARTIFACTS}/validation.json"}
    if not required_bindings <= freeze.get("artifact_sha256", {}).keys():
        raise ValueError("Required helper or validation bindings are missing")
    for name, expected in frozen_inventory.items():
        if sha(regular_file(root, f"{ARTIFACTS}/source/{name}")) != expected:
            raise ValueError("Frozen source bytes differ")
    for name, expected in freeze["artifact_sha256"].items():
        if sha(regular_file(root, name)) != expected:
            raise ValueError("Bound artifact changed")
    if sha(regular_file(root, "target/debug/s-code-daemon")) != freeze["daemon_sha256"]:
        raise ValueError("Candidate executable differs from the freeze")
    if sha(Path(sys.executable).resolve()) != freeze["python_sha256"] or platform.python_version() != freeze["python_version"]:
        raise ValueError("Python interpreter differs from the freeze")
    return True


def approval(root, freeze_path):
    record = read(regular_file(root, APPROVAL))
    if record.get("decision") != "approved" or record.get("freeze_sha256") != sha(freeze_path):
        raise ValueError("An independent approval of the exact actual freeze is required")
    if not isinstance(record.get("reviewer"), str) or not record["reviewer"].strip():
        raise ValueError("Independent approval must identify its reviewer")
    return sha(root / APPROVAL)
