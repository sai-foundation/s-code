"""Private prospective quality08 bindings. No daemon, grader, or model invocation."""
from __future__ import annotations
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys

ROUND = "quality08"
FAMILIES = ("flow", "queue", "report")
ARMS = ("off", "raw", "learned")
SEEDS = (17, 29, 43)
HELPERS = ("quality08_common.py", "prepare-quality08-freeze.py", "run-quality08.py", "analyze-quality08.py", "test-quality08-helpers.py", "test-quality08-analysis.py")
BENCHMARK = "tests/benchmarks/self-evolving"
ARTIFACTS = ".work/quality08-freeze-artifacts"
FREEZE = ".work/quality08-development-freeze.json"
APPROVAL = ".work/quality08-freeze-independent-review.json"
PIPELINE = ".work/quality08-pipeline.json"
LAUNCH = ".work/quality08-launch.json"
TRANSFER = ".work/quality08-transfer"
TRANSFER_FREEZE = ".work/quality08-transfer-freeze.json"
RELEASE = ".work/quality08-release"
TASKS_SHA = "5e5f3fe2e9ae01a6614ffada9025caadf4cd5ce609b3e1236df71e2be0b5c7d1"
GRADER_SHA = "aca60aeda201651e225f043112e21921644f78081c03f9a0c1a8fd2ae7184918"


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


def task_metadata():
    return [{"task_id": f"{prefix}{number}", "family": family, "negative_control": number == 4}
            for prefix, family in (("F", "flow"), ("Q", "queue"), ("R", "report")) for number in range(1, 5)]


def protocol():
    return dict(arms=list(ARMS), seeds=list(SEEDS), primary_horizon=12, bootstrap_samples=10000,
                bootstrap_seed=20260905, target_reduction=.20)


def schedule():
    # Import the unchanged, source-bound schedule implementation; no task prompts.
    import importlib.util
    path = Path(__file__).resolve().parents[1] / BENCHMARK / "analysis.py"
    spec = importlib.util.spec_from_file_location("quality08_numerical_schedule", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.schedule({"protocol": {**protocol(), "task_manifest": task_metadata()}})


def configuration():
    return dict(round=ROUND, phase="exposed-development", evidence_class="exposed-development",
        model="z-ai/glm-5.3", provider="z-ai/fp8", fallback=False, reasoning_effort="low", temperature=0,
        training_seed=17, seeds=list(SEEDS), family_order=list(FAMILIES), schedule=schedule(),
        task_metadata=task_metadata(), protocol=protocol(),
        global_admission_cap_usd=45, training_task_cost_cap_usd=1.5, training_reserved_cap_usd=4.5,
        transfer_cost_cap_usd=40.5, task_cost_cap_usd=1.5, proxy_calls_cap_per_training=200,
        proxy_calls_cap_transfer=21600, agent_dispatch_cap_per_task=48, tool_call_cap_per_task=64,
        task_token_cap=1_000_000, task_outer_timeout_seconds=900, coding_initial_max_output_tokens=8192,
        coding_adaptive_max_output_tokens=32768, reflection_model_calls_expected=0,
        planned_training_attempts=3, planned_development_attempts=108,
        raw_control="unchanged raw_corpus/raw_retrieve; different selection and representation boundary",
        development_screen=dict(horizon=12, minimum_token_reduction_vs_off=.20,
            no_observed_success_loss=True, known_grading_and_usage=True, known_dollar_costs=True,
            no_budget_denials=True, minimum_source_exposure_requests=1,
            minimum_eligible_frozen_observations_per_family=1, required_initial_positive_exposure_requests=27),
        confirmatory_claim=False, automatic_reveal_authorized=False,
        independent_advance_decision_required=True, terminal_learning_evidence_required=True,
        planned_artifacts=dict(approval=APPROVAL, launch=LAUNCH, completion=PIPELINE,
            transfer_freeze=TRANSFER_FREEZE, transfer_output=TRANSFER, release=RELEASE,
            analysis=".work/quality08-reviewed/quality08.json"),
        selection="All 111 slots retained. Train three families once; any failed/incomplete training prevents all transfer. No rerun, replacement, pooling or budget topup.")


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
        if json.dumps(freeze.get(key), sort_keys=True, allow_nan=False) != json.dumps(expected, sort_keys=True, allow_nan=False):
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
        if not isinstance(name, str) or Path(name).name != name or not name.endswith(".json") or name in seen or name in {"quality08.json", "independent-review.json"}:
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
            ("quality05", "development"), ("quality05", "independent_review"),
            ("quality06", "development"), ("quality06", "independent_review"),
            ("quality07", "development"), ("quality07", "independent_review")} <= kinds:
        raise ValueError("Retain quality04 phases and quality05/quality06/quality07 development/audits separately")
    if len(rows) != 14:
        raise ValueError("Retain the prior twelve histories plus the quality07 report/review")
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
    if sha(regular_file(root, f"{RELEASE}/tasks.json")) != TASKS_SHA or sha(regular_file(root, f"{RELEASE}/grader.py")) != GRADER_SHA:
        raise ValueError("Previously exposed task or oracle bytes changed")
    return True


def approval(root, freeze_path):
    record = read(regular_file(root, APPROVAL))
    if record.get("decision") != "approved" or record.get("freeze_sha256") != sha(freeze_path):
        raise ValueError("An independent approval of the exact actual freeze is required")
    if not isinstance(record.get("reviewer"), str) or not record["reviewer"].strip():
        raise ValueError("Independent approval must identify its reviewer")
    return sha(root / APPROVAL)
