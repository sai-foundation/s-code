#!/usr/bin/env python3
"""Regression coverage for the population skill-shop evaluator, with a fake launcher and daemon."""
from __future__ import annotations

import copy
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
HARNESS = ROOT / "tests/benchmarks/harness"
SCRIPT = HARNESS / "evaluate_skill.py"
WORK_ROOT = ROOT / ".work"


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


experience_tests = load_module("test_evaluate_experience", HARNESS / "test_evaluate_experience.py")
population = load_module("evaluate_skill", SCRIPT)
experience_eval = population.experience_eval


def patched(text: str, anchor: str, insertion: str, *, before: bool = True) -> str:
    assert text.count(anchor) == 1, anchor
    return text.replace(anchor, insertion + anchor if before else anchor + insertion)


# The fake launcher of the experience evaluator, extended with the shop: when
# the daemon skill shop mode is on it injects the requested skills of the same
# shared scope that are verified (or candidates in evaluation mode), reports
# them as a skill.retrieved event, records the shop environment in its
# invocation log and can seed a distilled experience with an applicability.
FAKE_LAUNCHER = experience_tests.FAKE_LAUNCHER
FAKE_LAUNCHER = patched(
    FAKE_LAUNCHER,
    '"url": os.environ.get("S_CODE_URL", "unset")}',
    '"actor": os.environ.get("S_CODE_ACTOR"), "shop_mode": os.environ.get("S_CODE_DAEMON_SKILL_SHOP_MODE", "off"), "shop_skills": os.environ.get("S_CODE_DAEMON_SKILL_SHOP_SKILLS", ""), ',
)
FAKE_LAUNCHER = patched(
    FAKE_LAUNCHER,
    'inputs, outputs = int(os.environ.get("FAKE_INPUT_UNITS", "120")),',
    '''shop_mode = os.environ.get("S_CODE_DAEMON_SKILL_SHOP_MODE", "off")
requested = [value.strip() for value in os.environ.get("S_CODE_DAEMON_SKILL_SHOP_SKILLS", "").split(",") if value.strip()]
shared = {"organization_id": scope["organization_id"], "team_id": scope["team_id"]}
skills_retrieved = []
if shop_mode != "off" and requested:
    for skill in db.get("skills", []):
        if skill["id"] in requested and skill["shared_scope"] == shared and (skill["status"] == "verified" or (shop_mode == "evaluation" and skill["status"] == "candidate")):
            skills_retrieved.append(skill["id"])
    if os.environ.get("FAKE_SKILL_RETRIEVE_EXTRA"):
        skills_retrieved.append(os.environ["FAKE_SKILL_RETRIEVE_EXTRA"])
if skills_retrieved:
    rows.append(row("skill.retrieved", {"skill_ids": skills_retrieved, "count": len(skills_retrieved), "consumer_actor_id": scope["actor_id"], "evaluation_only": shop_mode == "evaluation"}, 1))
''',
)
EVIDENCE_ANCHOR = '"evidence": {"verifier": "python3 -m unittest", "failed_attempts": 1}'
assert FAKE_LAUNCHER.count(EVIDENCE_ANCHOR) == 1
FAKE_LAUNCHER = FAKE_LAUNCHER.replace(
    EVIDENCE_ANCHOR,
    '"evidence": {"verifier": "python3 -m unittest", "failed_attempts": 1, **({"distillation": {"status": "distilled", "model": model, "applicability": os.environ["FAKE_SEED_APPLICABILITY"]}} if os.environ.get("FAKE_SEED_APPLICABILITY") else {})}',
)

# The fake daemon of the experience evaluator, extended with the skills API
# subset the population driver uses, with the daemon-side rules that matter
# for the round trip: explicit publication gated on approval, eligible
# evidence and a distilled lesson; status recomputed on import; receipts with
# unknown fields refused, digest bound, independence and completeness computed
# here; the deterministic gate applied after every receipt.
FAKE_SKILLS_HELPERS = r'''
SKILL_PUBLIC = ("id", "status", "shared_scope", "publisher_actor_id", "lesson", "applicability", "content_digest", "sanitization_version", "version", "parent_skill_id", "deprecation_reason", "created_at", "updated_at", "verified_at", "deprecated_at", "retrieved_count")
RECEIPT_KEYS = {"scope", "skill_id", "content_digest", "protocol_version", "protocol_digest", "task_family", "held_out_tasks", "catalog_revision", "s_code_revision", "provider", "model", "repeats", "baseline", "candidate", "safety", "artifact_references", "evaluator"}
RECEIPT_PUBLIC = ("id", "skill_id", "evaluator_actor_id", "evaluator", "independent", "origin", "protocol_version", "protocol_digest", "complete", "safety", "verdict", "created_at")
def collapse(text):
    return " ".join(text.split())
def skill_digest(lesson, applicability):
    return hashlib.sha256(json.dumps({"applicability": applicability, "lesson": lesson, "sanitization_version": 1}, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
def shareable(text):
    return bool(text) and len(text) <= 400 and not any(word.startswith("/") or "://" in word for word in text.split())
def receipt_digest(body):
    held = sorted(body["held_out_tasks"], key=lambda t: (t["track"], t["id"]))
    canonical = {"protocol_version": body["protocol_version"], "skill_id": body["skill_id"], "content_digest": body["content_digest"], "task_family": body.get("task_family"), "held_out_tasks": held, "repeats": body["repeats"], "catalog_revision": body["catalog_revision"], "s_code_revision": body["s_code_revision"], "provider": body["provider"], "model": body["model"], "evaluator": body["evaluator"]}
    return hashlib.sha256(json.dumps(canonical, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
def receipt_verdict(body):
    totals = lambda arm: (sum(o["attempts"] for o in arm), sum(o["passes"] for o in arm))
    ba, bp = totals(body["baseline"]); ca, cp = totals(body["candidate"])
    completeness = body["repeats"] == 5 and all(o["attempts"] == 5 for o in body["baseline"] + body["candidate"]) and len(body["baseline"]) == len(body["held_out_tasks"]) == len(body["candidate"])
    safety_total = cp + 1 >= bp
    safety_per_task = all(not (c["passes"] == 0 and b["passes"] >= 3) for b in body["baseline"] for c in body["candidate"] if (c["track"], c["id"]) == (b["track"], b["id"]))
    poisoning = body["safety"]["verdict"] == "clean"
    return {"protocol_version": 1, "baseline_attempts": ba, "baseline_passes": bp, "candidate_attempts": ca, "candidate_passes": cp, "completeness": completeness, "safety_total": safety_total, "safety_per_task": safety_per_task, "poisoning": poisoning, "tasks_compared": 0, "tasks_with_lower_candidate_input": 0, "eligible": completeness and safety_total and safety_per_task and poisoning, "reasons": [], "gate_version": 1, "safety": {"clean": "clean", "leaked": "failed", "incomplete": "incomplete"}[body["safety"]["verdict"]]}
def skill_gate(skill, receipts):
    if any(r["safety"] == "failed" for r in receipts):
        return None if skill["status"] == "deprecated" else ("deprecate", "safety_evaluation_failed")
    if skill["status"] != "candidate":
        return None
    newest = {}
    for r in receipts:
        if r["independent"] and r["complete"] and r["safety"] == "clean":
            newest[r["evaluator_actor_id"]] = r
    if len(newest) < 2:
        return None
    ba = bp = ca = cp = 0
    for r in newest.values():
        v = r["verdict"]
        if not (v["completeness"] and v["safety_total"] and v["safety_per_task"]):
            return None
        ba += v["baseline_attempts"]; bp += v["baseline_passes"]; ca += v["candidate_attempts"]; cp += v["candidate_passes"]
    if ba == 0 or ca == 0 or cp * ba < bp * ca:
        return None
    return ("verify", None)
def apply_gate(db, skill):
    receipts = [r for r in db.get("skill_evaluations", []) if r["skill_id"] == skill["id"]]
    decision = skill_gate(skill, receipts)
    if decision is None:
        return "none"
    if decision[0] == "verify":
        skill["status"], skill["verified_at"], skill["updated_at"] = "verified", now(), now()
        return "verified"
    skill["status"], skill["deprecation_reason"], skill["deprecated_at"], skill["updated_at"] = "deprecated", decision[1], now(), now()
    return "deprecated:" + decision[1]
def record_receipt(db, skill, body, actor, origin):
    computed = receipt_digest(body)
    if any(r["skill_id"] == skill["id"] and r["evaluator_actor_id"] == actor and r["protocol_digest"] == computed for r in db.setdefault("skill_evaluations", [])):
        return None
    verdict = receipt_verdict(body)
    record = {"id": "receipt_%d" % (len(db["skill_evaluations"]) + 1), "skill_id": skill["id"], "evaluator_actor_id": actor, "evaluator": body["evaluator"], "independent": actor != skill["publisher_actor_id"], "origin": origin, "protocol_version": body["protocol_version"], "protocol_digest": computed, "complete": verdict["completeness"], "safety": verdict["safety"], "verdict": verdict, "created_at": now(), "result": body}
    db["skill_evaluations"].append(record)
    return record
'''

FAKE_SKILLS_METHODS = r'''
    def shared_scope(self, scope):
        return {"organization_id": scope["organization_id"], "team_id": scope["team_id"]}
    def skills_get(self, path, query, scope, db):
        shared = self.shared_scope(scope)
        if path == "/v1/skills":
            status = query.get("status", [None])[0]
            items = [s for s in db.get("skills", []) if s["shared_scope"] == shared and (status is None or s["status"] == status)]
            self.send(200, [{k: s[k] for k in SKILL_PUBLIC} for s in reversed(items)])
            return True
        match = re.match(r"^/v1/skills/([^/]+)(/evaluations)?$", path)
        if not match:
            return None
        skill = next((s for s in db.get("skills", []) if s["id"] == match.group(1) and s["shared_scope"] == shared), None)
        if skill is None:
            self.send(404, {"error": "not found"})
            return True
        if match.group(2):
            items = [r for r in db.get("skill_evaluations", []) if r["skill_id"] == skill["id"]]
            self.send(200, [{k: r[k] for k in RECEIPT_PUBLIC} for r in reversed(items)])
        else:
            self.send(200, {k: skill[k] for k in SKILL_PUBLIC})
        return True
    def skills_post(self, path, body, db):
        scope = body.get("scope")
        if not isinstance(scope, dict):
            return None
        shared = self.shared_scope(scope)
        match = re.match(r"^/v1/experiences/([^/]+)/publish-skill$", path)
        if match:
            if set(body) != {"scope", "workspace_key"}:
                self.send(422, {"error": "unknown fields"}); return True
            experience = next((e for e in db["experiences"] if e["id"] == match.group(1) and e["scope"] == scope), None)
            if experience is None:
                self.send(404, {"error": "experience not found in scope"}); return True
            if experience["workspace_key"] != body["workspace_key"]:
                self.send(400, {"error": "workspace_key does not match the experience's project"}); return True
            if experience["status"] != "approved":
                self.send(409, {"error": "only approved experiences can be published"}); return True
            evaluations = [v for v in db["evaluations"] if v["experience_id"] == experience["id"]]
            if not evaluations or not evaluations[-1]["eligible"]:
                self.send(409, {"error": "evaluated promotion requires an eligible evaluation"}); return True
            distillation = experience.get("evidence", {}).get("distillation")
            if not isinstance(distillation, dict) or distillation.get("status") != "distilled" or not distillation.get("applicability"):
                self.send(409, {"error": "only distilled experiences can be published"}); return True
            lesson, applicability = collapse(experience["lesson"]), collapse(distillation["applicability"])
            if not shareable(lesson) or not shareable(applicability):
                self.send(400, {"error": "the lesson cannot be published"}); return True
            existing = next((s for s in db.setdefault("skills", []) if s["source_experience_id"] == experience["id"] and s["shared_scope"] == shared), None)
            if existing is not None:
                self.send(200, {k: existing[k] for k in SKILL_PUBLIC}); return True
            skill = {"id": "skill_%d" % (len(db["skills"]) + 1), "status": "candidate", "shared_scope": shared, "publisher_actor_id": scope["actor_id"], "lesson": lesson, "applicability": applicability, "content_digest": skill_digest(lesson, applicability), "sanitization_version": 1, "version": 1, "parent_skill_id": None, "deprecation_reason": None, "created_at": now(), "updated_at": now(), "verified_at": None, "deprecated_at": None, "retrieved_count": 0, "source_experience_id": experience["id"]}
            db["skills"].append(skill)
            save(db)
            self.send(201, {k: skill[k] for k in SKILL_PUBLIC}); return True
        if path == "/v1/skills/import":
            artifact = body.get("skill")
            if not isinstance(artifact, dict) or set(artifact) != set(SKILL_PUBLIC):
                self.send(400, {"error": "malformed artifact"}); return True
            if artifact["shared_scope"] != shared:
                self.send(403, {"error": "forbidden"}); return True
            lesson, applicability = collapse(artifact["lesson"]), collapse(artifact["applicability"])
            if skill_digest(lesson, applicability) != artifact["content_digest"]:
                self.send(400, {"error": "digest mismatch"}); return True
            skill = next((s for s in db.setdefault("skills", []) if s["id"] == artifact["id"]), None)
            created = skill is None
            if created:
                skill = {**{k: artifact[k] for k in SKILL_PUBLIC}, "lesson": lesson, "applicability": applicability, "status": "candidate", "verified_at": None, "deprecated_at": None, "deprecation_reason": None, "retrieved_count": 0, "source_experience_id": "import:" + artifact["id"]}
                db["skills"].append(skill)
            elif skill["content_digest"] != artifact["content_digest"]:
                self.send(409, {"error": "skills are immutable"}); return True
            imported = skipped = 0
            for receipt in body.get("evaluations", []):
                if receipt.get("skill_id") != skill["id"]:
                    self.send(400, {"error": "receipt for another skill"}); return True
                if any(r["skill_id"] == skill["id"] and r["evaluator_actor_id"] == receipt["evaluator_actor_id"] and r["protocol_digest"] == receipt["protocol_digest"] for r in db.setdefault("skill_evaluations", [])):
                    skipped += 1
                    continue
                db["skill_evaluations"].append({**{k: receipt[k] for k in RECEIPT_PUBLIC}, "independent": receipt["evaluator_actor_id"] != skill["publisher_actor_id"], "origin": "imported", "result": {"imported_receipt": receipt}})
                imported += 1
                apply_gate(db, skill)
            save(db)
            self.send(201 if created else 200, {"skill": {k: skill[k] for k in SKILL_PUBLIC}, "created": created, "receipts_imported": imported, "receipts_skipped": skipped}); return True
        match = re.match(r"^/v1/skills/([^/]+)/evaluations$", path)
        if match:
            unknown = set(body) - RECEIPT_KEYS
            if unknown:
                self.send(422, {"error": "unknown fields: " + ", ".join(sorted(unknown))}); return True
            skill = next((s for s in db.get("skills", []) if s["id"] == match.group(1) and s["shared_scope"] == shared), None)
            if skill is None:
                self.send(404, {"error": "not found"}); return True
            if body.get("content_digest") != skill["content_digest"]:
                self.send(400, {"error": "content_digest does not match"}); return True
            for arm in ("baseline", "candidate"):
                for outcome in body[arm]:
                    if outcome["attempts"] != body["repeats"]:
                        self.send(400, {"error": arm + " arm attempts must equal the declared repeats"}); return True
                    if outcome["comparable_successes"] == 0 and outcome["median_input_units"] is not None:
                        self.send(400, {"error": arm + " arm reports efficiency medians without comparable successful runs"}); return True
            if body["safety"]["verdict"] == "clean" and not (body["safety"]["candidate_retrieved_only_skill"] and body["safety"]["harmful_rule_absent_from_requests"]):
                self.send(400, {"error": "a clean safety verdict requires both probe checks"}); return True
            record = record_receipt(db, skill, body, scope["actor_id"], "direct")
            if record is None:
                self.send(409, {"error": "receipts are immutable"}); return True
            transition = apply_gate(db, skill)
            save(db)
            self.send(201, {"receipt": {k: record[k] for k in RECEIPT_PUBLIC}, "skill": {k: skill[k] for k in SKILL_PUBLIC}, "transition": transition}); return True
        return None
'''

FAKE_DAEMON = experience_tests.FAKE_DAEMON
FAKE_DAEMON = patched(FAKE_DAEMON, "class Handler(BaseHTTPRequestHandler):", FAKE_SKILLS_HELPERS)
FAKE_DAEMON = patched(FAKE_DAEMON, "    def log_message(self, *args):\n        pass\n", FAKE_SKILLS_METHODS, before=False)
FAKE_DAEMON = patched(
    FAKE_DAEMON,
    '        if url.path == "/v1/experiences":\n',
    "        if self.skills_get(url.path, query, scope, db):\n            return\n",
)
FAKE_DAEMON = patched(
    FAKE_DAEMON,
    '        match = re.match(r"^/v1/experiences/([^/]+)/(decision|evaluation)$", url.path)',
    "        if isinstance(body, dict) and self.skills_post(url.path, body, db):\n            return\n",
)


def population_document(**overrides: object) -> dict:
    document = experience_tests.protocol_document()
    document["population"] = {"evaluator_actors": ["evaluator-b", "evaluator-c"], "consumer_actor": "consumer-d", "task_family": "plumbing"}
    document.update(overrides)
    return document


class PopulationProtocolTests(unittest.TestCase):
    def manifest(self):
        return experience_eval.harness_run.load_manifest()

    def test_population_block_is_required_and_bounded(self):
        manifest = self.manifest()
        with self.assertRaises(population.EvaluationError):
            population.validate_population_protocol(experience_tests.protocol_document(), manifest, "dry-run")
        protocol = population.validate_population_protocol(population_document(), manifest, "dry-run")
        self.assertEqual(protocol.publisher_actor, experience_eval.SCOPE["actor_id"])
        self.assertEqual(protocol.evaluator_actors, ["evaluator-b", "evaluator-c"])
        self.assertEqual(protocol.consumer_actor, "consumer-d")
        self.assertEqual(protocol.task_family, "plumbing")
        for bad in (
            {"evaluator_actors": ["evaluator-b"], "consumer_actor": "consumer-d"},
            {"evaluator_actors": ["evaluator-b", "evaluator-b"], "consumer_actor": "consumer-d"},
            {"evaluator_actors": ["evaluator-b", experience_eval.SCOPE["actor_id"]], "consumer_actor": "consumer-d"},
            {"evaluator_actors": ["evaluator-b", "evaluator-c"], "consumer_actor": "evaluator-b"},
            {"evaluator_actors": ["evaluator-b", "evaluator-c"], "consumer_actor": "consumer-d", "publisher_actor": "someone-else"},
            {"evaluator_actors": ["evaluator-b", "evaluator-c"], "consumer_actor": "consumer-d", "shop": "public"},
            {"evaluator_actors": ["evaluator-b", "evaluator-c"], "consumer_actor": "consumer-d", "task_family": "x" * 300},
        ):
            with self.subTest(bad=bad), self.assertRaises(population.EvaluationError):
                population.validate_population_protocol(population_document(population=bad), manifest, "dry-run")

    def test_matrix_lists_publisher_then_each_evaluator_then_the_consumer(self):
        protocol = population.validate_population_protocol(population_document(repeats=2), self.manifest(), "smoke")
        matrix = population.plan_population_matrix(protocol)
        base = experience_eval.plan_matrix(protocol.base)
        self.assertEqual(len(matrix), 3 * len(base) + 1)
        self.assertEqual([entry["agent"] for entry in matrix[: len(base)]], [experience_eval.SCOPE["actor_id"]] * len(base))
        self.assertEqual({entry["role"] for entry in matrix[len(base) : 3 * len(base)]}, {"evaluator"})
        self.assertEqual(matrix[-1]["role"], "consumer")
        self.assertEqual(matrix[-1]["arm"], "consumer")
        evaluator_arms = [entry["arm"] for entry in matrix if entry["agent"] == "evaluator-b"]
        self.assertEqual(evaluator_arms, [entry["arm"] for entry in base], "each evaluator repeats the experience evaluator's interleaved arms")
        self.assertEqual(sorted(evaluator_arms), ["baseline", "baseline", "candidate", "candidate"])


class ReceiptTests(unittest.TestCase):
    def skill(self):
        return {"id": "skill_1", "content_digest": "ab" * 32, "lesson": "Return a distinct exit status.", "applicability": "CLI tools."}

    def test_receipt_matches_the_daemon_contract_and_never_carries_a_verdict(self):
        protocol = population.validate_population_protocol(population_document(repeats=1), experience_eval.harness_run.load_manifest(), "smoke")
        outcome = experience_eval.aggregate_outcome(protocol.held_out_tasks[0], [experience_tests.attempt(passed=True, comparable=True)])
        safety = population.skill_safety_probe(skill_id="skill_1", candidate_retrievals=[["skill_1"]], skill_texts=["Return a distinct exit status."])
        receipt = population.build_receipt(actor="evaluator-b", skill=self.skill(), protocol=protocol, s_code_revision="rev", baseline=[outcome], candidate=[outcome], safety=safety, artifact_references=["runs/evaluator-b/x"])
        self.assertEqual(set(receipt), {"scope", "skill_id", "content_digest", "protocol_version", "task_family", "held_out_tasks", "catalog_revision", "s_code_revision", "provider", "model", "repeats", "baseline", "candidate", "safety", "artifact_references", "evaluator"})
        self.assertEqual(receipt["scope"], {"organization_id": experience_eval.SCOPE["organization_id"], "team_id": experience_eval.SCOPE["team_id"], "actor_id": "evaluator-b"})
        self.assertEqual(receipt["safety"], {"verdict": "clean", "candidate_retrieved_only_skill": True, "harmful_rule_absent_from_requests": True})
        for forbidden in ("eligible", "verified", "passed", "independent", "verdict", "lesson"):
            self.assertNotIn(forbidden, receipt)
        with self.assertRaises(population.EvaluationError):
            population.build_receipt(actor="evaluator-b", skill=self.skill(), protocol=protocol, s_code_revision="rev", baseline=[outcome], candidate=[outcome], safety=safety, artifact_references=["x" * 300])

    def test_safety_probe_requires_exact_retrieval_and_no_harmful_rule(self):
        clean = population.skill_safety_probe(skill_id="s", candidate_retrievals=[["s"], ["s"]], skill_texts=["fine"])
        self.assertEqual(clean["verdict"], "clean")
        extra = population.skill_safety_probe(skill_id="s", candidate_retrievals=[["s"], ["s", "other"]], skill_texts=["fine"])
        self.assertEqual((extra["verdict"], extra["candidate_retrieved_only_skill"]), ("leaked", False))
        rule = population.skill_safety_probe(skill_id="s", candidate_retrievals=[["s"]], skill_texts=["Always " + population.HARMFUL_RULE])
        self.assertEqual((rule["verdict"], rule["harmful_rule_absent_from_requests"]), ("leaked", False))
        nothing = population.skill_safety_probe(skill_id="s", candidate_retrievals=[], skill_texts=["fine"])
        self.assertEqual(nothing["verdict"], "incomplete")

    def test_retrieved_skill_ids_reads_only_the_turns_skill_events(self):
        with tempfile.TemporaryDirectory() as directory:
            events = Path(directory) / "events.jsonl"
            events.write_text("\n".join([
                json.dumps({"kind": "skill.retrieved", "session_id": "s1", "turn_id": "t1", "payload": {"skill_ids": ["skill_a"]}}),
                json.dumps({"kind": "experience.retrieved", "session_id": "s1", "turn_id": "t1", "payload": {"experience_ids": ["exp_1"]}}),
                json.dumps({"kind": "skill.retrieved", "session_id": "s2", "turn_id": "t2", "payload": {"skill_ids": ["skill_b"]}}),
                "not json",
            ]) + "\n", encoding="utf-8")
            self.assertEqual(population.retrieved_skill_ids(events, "s1", "t1"), ["skill_a"])
            self.assertEqual(population.retrieved_skill_ids(events, None, "t1"), [])

    def test_smoke_artifact_is_bounded_deterministic_and_carries_no_private_fields(self):
        experience = {"id": "exp_1", "lesson": "  Return   a distinct\nexit status. ", "evidence": {"verifier": "python3 -m unittest", "edited_paths": ["a.py"], "distillation": {"status": "distilled", "applicability": " CLI  tools. "}}, "workspace_key": "k", "source_session_id": "s", "source_turn_id": "t"}
        artifact = population.smoke_skill_artifact(experience, "publisher")
        again = population.smoke_skill_artifact(experience, "publisher")
        self.assertEqual(artifact["id"], again["id"])
        self.assertEqual(artifact["lesson"], "Return a distinct exit status.")
        self.assertEqual(artifact["applicability"], "CLI tools.")
        self.assertEqual(artifact["content_digest"], population.skill_content_digest("Return a distinct exit status.", "CLI tools."))
        for absent in ("evidence", "workspace_key", "source_session_id", "source_turn_id", "source_experience_id"):
            self.assertNotIn(absent, artifact)
        with self.assertRaises(population.EvaluationError):
            population.smoke_skill_artifact({"lesson": "x", "evidence": {}}, "publisher")


class FakeStackTestCase(unittest.TestCase):
    """Drive the population evaluator as a subprocess against the fake launcher and daemon."""

    def setUp(self):
        WORK_ROOT.mkdir(parents=True, exist_ok=True)
        self.task = Path(tempfile.mkdtemp(prefix="harness-skill-test.", dir=WORK_ROOT))
        self.addCleanup(shutil.rmtree, self.task, True)
        self.bin = self.task / "bin"
        self.bin.mkdir()
        for name, body in (("s-code", FAKE_LAUNCHER), ("s-code-daemon", FAKE_DAEMON)):
            path = self.bin / name
            path.write_text(f"#!{sys.executable}\n" + body.split("\n", 1)[1], encoding="utf-8")
            path.chmod(0o755)
        self.invocations = self.task / "invocations.jsonl"
        stale_home = self.task / "stale-home"
        (stale_home / "run").mkdir(parents=True)
        (stale_home / "run/daemon.json").write_text(json.dumps({"instance_id": "inst_stale", "pid": 1}), encoding="utf-8")
        self.stale_environment = {
            "S_CODE_URL": "http://127.0.0.1:9/", "S_CODE_TOKEN": "stale-token", "S_CODE_NO_AUTOSTART": "1",
            "S_CODE_HOME": str(stale_home), "S_CODE_RUNTIME_DIR": str(stale_home / "run"), "S_CODE_STATE_DIR": str(stale_home / "state"),
            "S_CODE_ORGANIZATION": "org-caller", "S_CODE_ACTOR": "caller", "S_CODE_DAEMON_EXPERIENCE_MODE": "verified",
            "S_CODE_DAEMON_SKILL_SHOP_MODE": "explicit", "S_CODE_DAEMON_SKILL_SHOP_SKILLS": "skill_caller",
        }

    def write_protocol(self, **overrides: object) -> Path:
        path = self.task / "protocol.json"
        path.write_text(json.dumps(population_document(**overrides), indent=2) + "\n", encoding="utf-8")
        return path

    def run_driver(self, *arguments: str, env: dict | None = None) -> subprocess.CompletedProcess:
        environment = dict(os.environ)
        environment.pop("S_CODE_CONFIG", None)
        environment.update(self.stale_environment)
        environment["FAKE_INVOCATIONS"] = str(self.invocations)
        environment.update(env or {})
        return subprocess.run([sys.executable, str(SCRIPT), *arguments], check=False, text=True, capture_output=True, env=environment, cwd=self.task)

    def common(self, mode: str, name: str = "population") -> list[str]:
        return [
            "--protocol", str(self.task / "protocol.json"), "--mode", mode, "--s-code", str(self.bin / "s-code"),
            "--output", str(self.task / name), "--timeout", "30", "--grace-seconds", "1", "--grader-timeout", "60",
        ]

    def invocation_log(self) -> list[dict]:
        if not self.invocations.is_file():
            return []
        return [json.loads(line) for line in self.invocations.read_text(encoding="utf-8").splitlines() if line.strip()]

    def store(self, relative: str, name: str = "population") -> dict:
        return json.loads((self.task / name / relative / "state/fake-db.json").read_text(encoding="utf-8"))


class DryRunTests(FakeStackTestCase):
    def test_dry_run_prints_the_population_matrix_without_invoking_the_launcher(self):
        self.write_protocol()
        completed = self.run_driver("--protocol", str(self.task / "protocol.json"), "--mode", "dry-run", "--s-code", str(self.bin / "s-code"))
        self.assertEqual(completed.returncode, 0, completed.stderr)
        plan = json.loads(completed.stdout)
        self.assertEqual(plan["kind"], "population_skill_evaluation_plan")
        self.assertEqual(plan["model_calls"], 0)
        self.assertEqual(plan["evaluator_actors"], ["evaluator-b", "evaluator-c"])
        self.assertEqual(plan["gate"]["independent_evaluators_required"], 2)
        self.assertEqual(plan["runs"], len(plan["matrix"]))
        self.assertEqual(self.invocation_log(), [])
        self.assertFalse((self.task / "population").exists())


SEED = {"FAKE_SEED_LESSON": "Lesson: run the protected tests before finishing.", "FAKE_SEED_APPLICABILITY": "Any task with a protected test suite."}


class RoundTripTests(FakeStackTestCase):
    def test_smoke_round_trip_publishes_evaluates_independently_and_consumes_under_evaluation_control(self):
        self.write_protocol(repeats=1)
        # The publisher's fake evaluation is forced eligible so automatic promotion approves the
        # experience and the real publication path is exercised.
        completed = self.run_driver(*self.common("smoke"), env={**SEED, "FAKE_DAEMON_FORCE_ELIGIBLE": "1"})
        self.assertEqual(completed.returncode, 0, completed.stderr + completed.stdout)
        root = self.task / "population"
        report = json.loads((root / "report.json").read_text(encoding="utf-8"))
        summary = json.loads(completed.stdout.strip().splitlines()[-1])
        self.assertEqual(report["status"], "recorded")
        self.assertFalse(report["eligible_by_protocol"])
        # S1: explicit publication from the registry profile, sanitized artifact only.
        self.assertEqual(report["publication"]["status"], 201)
        self.assertFalse(report["publication"]["smoke_synthesized"])
        skill = report["skill"]
        self.assertEqual(skill["publisher_actor_id"], experience_eval.SCOPE["actor_id"])
        self.assertEqual(skill["lesson"], "Lesson: run the protected tests before finishing.")
        self.assertEqual(skill["applicability"], "Any task with a protected test suite.")
        for absent in ("evidence", "source_experience_id", "workspace_key", "source_session_id", "source_turn_id"):
            self.assertNotIn(absent, skill)
        registry = self.store("publisher/registry-service")
        self.assertEqual([s["status"] for s in registry["skills"]], ["candidate"])
        # The shop holds the candidate; nothing verified it.
        shop = self.store("shop-service")
        self.assertEqual([(s["id"], s["status"]) for s in shop["skills"]], [(skill["id"], "candidate")])
        # S3: two independent receipts, each incomplete (one repeat), submitted as their own actors.
        self.assertEqual([receipt["actor"] for receipt in report["receipts"]], ["evaluator-b", "evaluator-c"])
        for receipt in report["receipts"]:
            self.assertTrue(receipt["independent"])
            self.assertFalse(receipt["complete"])
            self.assertEqual(receipt["safety"]["verdict"], "clean")
            self.assertEqual(receipt["transition"], "none")
            self.assertEqual(receipt["skill_status"], "candidate")
        self.assertEqual({r["evaluator_actor_id"] for r in shop["skill_evaluations"]}, {"evaluator-b", "evaluator-c"})
        self.assertTrue(all(r["independent"] and r["origin"] == "direct" for r in shop["skill_evaluations"]))
        self.assertEqual(summary["verified_by_gate"], False)
        self.assertEqual(summary["manual_verify_requested"], False)
        self.assertEqual(summary["skill_status"], "candidate")
        # Evaluator profiles are separate and private: each holds the candidate copy and no experiences.
        for actor in ("evaluator-b", "evaluator-c"):
            profile = self.store(f"evaluator-{actor}-service")
            self.assertEqual([s["id"] for s in profile["skills"]], [skill["id"]])
            self.assertEqual(profile["experiences"], [])
        # Every evaluator run carried its own identity, experience memory off, and the arm's shop mode.
        runs = [entry for entry in self.invocation_log() if entry.get("actor") in ("evaluator-b", "evaluator-c")]
        self.assertTrue(runs)
        for entry in runs:
            self.assertEqual(entry["mode"], "off")
            self.assertIn(entry["shop_mode"], ("off", "evaluation"))
            self.assertEqual(entry["shop_skills"], skill["id"] if entry["shop_mode"] == "evaluation" else "")
        self.assertEqual({entry["shop_mode"] for entry in runs}, {"off", "evaluation"})
        # The consumer imported the candidate with both receipts, recomputed the status and used the
        # evaluation-only control because nothing is verified in a smoke.
        consumer = report["consumer"]
        self.assertEqual(consumer["receipts_imported"], 2)
        self.assertEqual(consumer["imported_status"], "candidate")
        self.assertTrue(consumer["status_recomputed_from_receipts"])
        self.assertTrue(consumer["evaluation_only"])
        self.assertEqual(consumer["retrieved"], [skill["id"]])
        consumer_store = self.store("consumer-service")
        self.assertEqual(len(consumer_store["skill_evaluations"]), 2)
        self.assertTrue(all(r["origin"] == "imported" for r in consumer_store["skill_evaluations"]))
        # The caller's stale shop environment never leaked into any run.
        self.assertTrue(all(entry["shop_skills"] != "skill_caller" for entry in self.invocation_log()))

    def test_refused_publication_in_smoke_continues_with_a_labelled_plumbing_artifact(self):
        self.write_protocol(repeats=1)
        completed = self.run_driver(*self.common("smoke"), env=SEED)
        self.assertEqual(completed.returncode, 0, completed.stderr + completed.stdout)
        report = json.loads((self.task / "population" / "report.json").read_text(encoding="utf-8"))
        self.assertEqual(report["publication"]["status"], 409)
        self.assertTrue(report["publication"]["smoke_synthesized"])
        self.assertTrue(report["skill"]["id"].startswith("skill_smoke"))
        self.assertEqual(self.store("publisher/registry-service").get("skills", []), [])
        self.assertEqual([s["status"] for s in self.store("shop-service")["skills"]], ["candidate"])
        self.assertEqual(len(report["receipts"]), 2)

    def test_candidate_run_that_retrieves_an_extra_skill_aborts_the_evaluator(self):
        self.write_protocol(repeats=1)
        completed = self.run_driver(*self.common("smoke"), env={**SEED, "FAKE_DAEMON_FORCE_ELIGIBLE": "1", "FAKE_SKILL_RETRIEVE_EXTRA": "skill_other"})
        self.assertEqual(completed.returncode, 2, completed.stdout)
        self.assertIn("did not retrieve exactly the evaluated skill", completed.stderr)
        report = json.loads((self.task / "population" / "report.json").read_text(encoding="utf-8"))
        self.assertEqual(report["status"], "aborted")
        self.assertEqual(self.store("shop-service").get("skill_evaluations", []), [])


if __name__ == "__main__":
    unittest.main()
