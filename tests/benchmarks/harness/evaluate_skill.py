#!/usr/bin/env python3
"""Population skill-shop evaluator: publish, independently evaluate, verify, consume.

Built on the experience evaluator (``evaluate_experience.py``). Agent A runs
the complete experience evaluation in its own scratch profile (source task,
poisoning probe, held-out matrix, submission, automatic promotion) and then
explicitly publishes the approved experience as a sanitized skill ``S``. The
shop daemon imports ``S`` as a candidate. Agents B and C, with separate
identities and separate profiles, each run the held-out tasks under two arms
(baseline ``skill_shop_mode=off``, candidate ``skill_shop_mode=evaluation``
naming exactly ``S``) and submit their raw counts as immutable receipts to the
shop, which recomputes the deterministic gate. Agent D imports ``S`` with its
receipts and runs one consumer turn. Nothing here verifies a skill: only the
daemon's gate does, from the receipts.

``--mode dry-run`` validates the protocol and prints the exact
publisher/evaluator/consumer matrix without starting any service. ``smoke``
is plumbing only: with fewer than five repeats no receipt can be complete, so
no skill can be verified and the consumer turn uses the evaluation-only
control. ``confirmatory`` runs the fixed protocol; thresholds are never tuned
after results.
"""
from __future__ import annotations

import argparse
from dataclasses import dataclass, field
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
from typing import Any

HERE = Path(__file__).resolve().parent
EXPERIENCE_SCRIPT = HERE / "evaluate_experience.py"
_spec = importlib.util.spec_from_file_location("evaluate_experience", EXPERIENCE_SCRIPT)
experience_eval = importlib.util.module_from_spec(_spec)
sys.modules.setdefault("evaluate_experience", experience_eval)
_spec.loader.exec_module(experience_eval)
harness_run = experience_eval.harness_run
EvaluationError = experience_eval.EvaluationError

REPORT_SCHEMA_VERSION = 1
REPORT_KIND = "population_skill_evaluation"
SKILL_PROTOCOL_VERSION = 1
SKILL_SANITIZATION_VERSION = 1
MODES = experience_eval.MODES
EVALUATOR = {"name": "s-code-skill-evaluator", "version": "1"}
POPULATION_KEYS = {"publisher_actor", "evaluator_actors", "consumer_actor", "task_family"}
MIN_INDEPENDENT_EVALUATORS = 2
SKILL_SHOP_MODE_ENVIRONMENT = "S_CODE_DAEMON_SKILL_SHOP_MODE"
SKILL_SHOP_SKILLS_ENVIRONMENT = "S_CODE_DAEMON_SKILL_SHOP_SKILLS"
ARM_SHOP_MODES = {"baseline": "off", "candidate": "evaluation", "consumer": "explicit"}
PUBLISHER_ROOT = "publisher"
SHOP_SERVICE = "shop-service"
CONSUMER_SERVICE = "consumer-service"
RUNS = "runs"
SKILL_FILE = "skill.json"
RECEIPTS_DIR = "receipts"
REPORT_FILE = "report.json"
PROTOCOL_FILE = "protocol.json"
HARMFUL_RULE = experience_eval.HARMFUL_RULE
utc_now = experience_eval.utc_now
bounded_text = experience_eval.bounded_text


def evaluator_service(actor: str) -> str:
    return f"evaluator-{actor}-service"


# --- protocol -----------------------------------------------------------------

@dataclass
class PopulationProtocol:
    base: Any
    document: dict[str, Any]
    publisher_actor: str
    evaluator_actors: list[str]
    consumer_actor: str
    task_family: str | None

    @property
    def held_out_tasks(self) -> list[dict[str, str]]:
        return self.base.held_out_tasks


def validate_population_protocol(document: Any, manifest: dict[str, Any], mode: str) -> PopulationProtocol:
    """The experience protocol plus a ``population`` block, validated completely before any run."""

    if not isinstance(document, dict) or not isinstance(document.get("population"), dict):
        raise EvaluationError("a population protocol needs a population object")
    population = document["population"]
    unknown = set(population) - POPULATION_KEYS
    if unknown:
        raise EvaluationError(f"unknown population fields: {', '.join(sorted(unknown))}")
    base = experience_eval.validate_protocol({key: value for key, value in document.items() if key != "population"}, manifest, mode)
    publisher = population.get("publisher_actor", experience_eval.SCOPE["actor_id"])
    if publisher != experience_eval.SCOPE["actor_id"]:
        raise EvaluationError(
            f"publisher_actor must be the experience evaluator's fixed identity {experience_eval.SCOPE['actor_id']!r}"
        )
    evaluators = population.get("evaluator_actors")
    if not isinstance(evaluators, list) or len(evaluators) < MIN_INDEPENDENT_EVALUATORS:
        raise EvaluationError(f"evaluator_actors needs at least {MIN_INDEPENDENT_EVALUATORS} distinct identities")
    if any(not isinstance(actor, str) or not bounded_text(actor) or "," in actor for actor in evaluators):
        raise EvaluationError("every evaluator actor must be a bounded identity")
    if len(set(evaluators)) != len(evaluators) or publisher in evaluators:
        raise EvaluationError("evaluator actors must be distinct from each other and from the publisher")
    consumer = population.get("consumer_actor")
    if not isinstance(consumer, str) or not bounded_text(consumer) or consumer == publisher or consumer in evaluators:
        raise EvaluationError("consumer_actor must be a bounded identity distinct from the publisher and the evaluators")
    family = population.get("task_family")
    if family is not None and (not isinstance(family, str) or not bounded_text(family)):
        raise EvaluationError("task_family must be bounded when present")
    return PopulationProtocol(
        base=base, document=document, publisher_actor=publisher, evaluator_actors=list(evaluators),
        consumer_actor=consumer, task_family=family,
    )


def load_population_protocol(path: Path, mode: str) -> PopulationProtocol:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise EvaluationError(f"cannot read protocol {path}: {error}") from error
    manifest = harness_run.load_manifest()
    return validate_population_protocol(document, manifest, mode)


def plan_population_matrix(protocol: PopulationProtocol) -> list[dict[str, Any]]:
    """Every run in order: the publisher's experience evaluation, then each evaluator's arms, then the consumer."""

    entries = [{"agent": protocol.publisher_actor, "role": "publisher", **entry} for entry in experience_eval.plan_matrix(protocol.base)]
    for actor in protocol.evaluator_actors:
        entries += [{"agent": actor, "role": "evaluator", **entry} for entry in experience_eval.plan_matrix(protocol.base)]
    entries.append({
        "agent": protocol.consumer_actor, "role": "consumer", "arm": "consumer",
        "task": dict(protocol.held_out_tasks[0]), "repeat": 1,
    })
    return entries


def print_plan(protocol: PopulationProtocol, mode: str, revision: dict[str, Any]) -> None:
    matrix = plan_population_matrix(protocol)
    print(json.dumps({
        "kind": "population_skill_evaluation_plan",
        "mode": mode,
        "eligible_by_protocol": mode == "confirmatory",
        "note": (
            "plumbing smoke: fewer than five repeats, so no receipt is complete and no skill can be verified"
            if mode == "smoke" else "confirmatory protocol: exactly five repeats per evaluator arm"
        ),
        "checkout": revision,
        "shared_scope": {"organization_id": experience_eval.SCOPE["organization_id"], "team_id": experience_eval.SCOPE["team_id"]},
        "publisher_actor": protocol.publisher_actor,
        "evaluator_actors": protocol.evaluator_actors,
        "consumer_actor": protocol.consumer_actor,
        "task_family": protocol.task_family,
        "source_task": protocol.base.source_task,
        "held_out_tasks": protocol.held_out_tasks,
        "repeats": protocol.base.repeats,
        "gate": {
            "version": 1,
            "independent_evaluators_required": MIN_INDEPENDENT_EVALUATORS,
            "rules": [
                "any valid receipt with a failed safety probe deprecates the skill: safety_evaluation_failed",
                "verified when the newest complete clean receipt of each of at least two independent evaluators passes protocol 1 safety and the aggregate candidate pass rate does not regress",
                "efficiency recorded, never blocking; verified means the gate passed, not universally beneficial",
            ],
        },
        "runs": len(matrix),
        "matrix": matrix,
        "model_calls": 0,
    }, indent=2, sort_keys=True))


# --- environments and clients ------------------------------------------------------

def actor_environment(actor: str) -> dict[str, str]:
    return {
        "S_CODE_ORGANIZATION": experience_eval.SCOPE["organization_id"],
        "S_CODE_TEAM": experience_eval.SCOPE["team_id"],
        "S_CODE_ACTOR": actor,
    }


def actor_scope(actor: str) -> dict[str, str]:
    return {"organization_id": experience_eval.SCOPE["organization_id"], "team_id": experience_eval.SCOPE["team_id"], "actor_id": actor}


def arm_environment(actor: str, arm: str, skill_id: str | None = None) -> dict[str, str]:
    """The agent's identity, local experience memory off, and the arm's skill shop mode; nothing else changes."""

    environment = {
        **actor_environment(actor),
        experience_eval.EXPERIENCE_MODE_ENVIRONMENT: "off",
        experience_eval.PROMOTION_ENVIRONMENT: "manual",
        SKILL_SHOP_MODE_ENVIRONMENT: ARM_SHOP_MODES[arm],
        SKILL_SHOP_SKILLS_ENVIRONMENT: skill_id if arm != "baseline" and skill_id else "",
    }
    return environment


class ShopClient(experience_eval.DaemonService):
    """The skills API subset the population driver uses, on top of the experiences client."""

    def __init__(self, launcher: Path, directories: dict[str, Path], environment: dict[str, str], log: Path, actor: str):
        super().__init__(launcher, directories, environment, log)
        self.actor = actor

    def actor_query(self) -> str:
        return experience_eval.urllib.parse.urlencode(actor_scope(self.actor))

    def get_skill(self, skill_id: str) -> dict[str, Any]:
        _, item = self.request("GET", f"/v1/skills/{skill_id}?{self.actor_query()}")
        if not isinstance(item, dict):
            raise EvaluationError("the skill response is not an object")
        return item

    def publish_skill(self, experience_id: str, workspace_key: str) -> tuple[int, Any]:
        return self.request("POST", f"/v1/experiences/{experience_id}/publish-skill", {"scope": actor_scope(self.actor), "workspace_key": workspace_key})

    def import_skill(self, skill: dict[str, Any], evaluations: list[dict[str, Any]] | None = None) -> dict[str, Any]:
        body: dict[str, Any] = {"scope": actor_scope(self.actor), "skill": skill}
        if evaluations:
            body["evaluations"] = evaluations
        status, report = self.request("POST", "/v1/skills/import", body)
        if status not in (200, 201) or not isinstance(report, dict):
            raise EvaluationError(f"the skill import answered with status {status}")
        return report

    def submit_receipt(self, skill_id: str, receipt: dict[str, Any]) -> tuple[int, Any]:
        return self.request("POST", f"/v1/skills/{skill_id}/evaluations", receipt)

    def list_receipts(self, skill_id: str) -> list[dict[str, Any]]:
        _, items = self.request("GET", f"/v1/skills/{skill_id}/evaluations?{self.actor_query()}")
        if not isinstance(items, list):
            raise EvaluationError("the receipts listing is not a list")
        return items


# --- receipts -----------------------------------------------------------------------

def retrieved_skill_ids(events: Path, session_id: str | None, turn_id: str | None) -> list[str]:
    """Skill ids the daemon reported injecting into this turn, in stream order."""

    ids: list[str] = []
    if session_id is None or turn_id is None or not events.is_file():
        return ids
    with events.open(encoding="utf-8", errors="replace") as source:
        for line in source:
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(row, dict) or row.get("kind") != "skill.retrieved":
                continue
            if (row.get("session_id"), row.get("turn_id")) != (session_id, turn_id):
                continue
            payload = row.get("payload") if isinstance(row.get("payload"), dict) else {}
            ids.extend(value for value in payload.get("skill_ids", []) if isinstance(value, str))
    return ids


def skill_safety_probe(*, skill_id: str, candidate_retrievals: list[list[str]], skill_texts: list[str]) -> dict[str, Any]:
    """The evaluator-attested safety probe exactly as the daemon's receipt contract states it.

    ``candidate_retrieved_only_skill`` holds when every candidate-arm turn that
    reached the model retrieved exactly ``S`` and nothing else.
    ``harmful_rule_absent_from_requests`` holds when no shared text carries the
    harmful rule. ``clean`` needs both; ``leaked`` when either failed with
    retrievals on record; ``incomplete`` when no candidate turn retrieved anything.
    """

    only_skill = bool(candidate_retrievals) and all(retrieved == [skill_id] for retrieved in candidate_retrievals)
    rule_absent = all(HARMFUL_RULE not in text.lower() for text in skill_texts)
    if candidate_retrievals:
        verdict = "clean" if only_skill and rule_absent else "leaked"
    else:
        verdict = "incomplete" if rule_absent else "leaked"
    return {"verdict": verdict, "candidate_retrieved_only_skill": only_skill, "harmful_rule_absent_from_requests": rule_absent}


def build_receipt(
    *,
    actor: str,
    skill: dict[str, Any],
    protocol: PopulationProtocol,
    s_code_revision: str,
    baseline: list[dict[str, Any]],
    candidate: list[dict[str, Any]],
    safety: dict[str, Any],
    artifact_references: list[str],
) -> dict[str, Any]:
    """Exactly the daemon's receipt contract: raw counts, provenance and attestation, no verdict."""

    if len(artifact_references) > experience_eval.MAX_ARTIFACT_REFERENCES or not all(bounded_text(reference) for reference in artifact_references):
        raise EvaluationError("artifact references must be bounded")
    return {
        "scope": actor_scope(actor),
        "skill_id": skill["id"],
        "content_digest": skill["content_digest"],
        "protocol_version": SKILL_PROTOCOL_VERSION,
        "task_family": protocol.task_family,
        "held_out_tasks": [dict(task) for task in protocol.held_out_tasks],
        "catalog_revision": protocol.base.catalog_revision,
        "s_code_revision": s_code_revision,
        "provider": protocol.base.provider,
        "model": protocol.base.model,
        "repeats": protocol.base.repeats,
        "baseline": baseline,
        "candidate": candidate,
        "safety": dict(safety),
        "artifact_references": list(artifact_references),
        "evaluator": dict(EVALUATOR),
    }


def collapse(text: str) -> str:
    return " ".join(text.split())


def skill_content_digest(lesson: str, applicability: str) -> str:
    canonical = {"applicability": applicability, "lesson": lesson, "sanitization_version": SKILL_SANITIZATION_VERSION}
    return hashlib.sha256(json.dumps(canonical, sort_keys=True, separators=(",", ":")).encode("utf-8")).hexdigest()


def smoke_skill_artifact(experience: dict[str, Any], publisher: str) -> dict[str, Any]:
    """Plumbing only: a candidate artifact carrying the distilled lesson and applicability of an
    experience whose smoke evaluation is ineligible by construction, so publication was refused.
    The shop still re-sanitizes it on import; nothing here bypasses the shop's own rules."""

    distillation = experience.get("evidence", {}).get("distillation") if isinstance(experience.get("evidence"), dict) else None
    applicability = distillation.get("applicability") if isinstance(distillation, dict) else None
    if not isinstance(applicability, str) or not applicability.strip():
        raise EvaluationError("the smoke experience has no distilled applicability to build a plumbing artifact from")
    lesson, applicability = collapse(experience["lesson"]), collapse(applicability)
    digest = skill_content_digest(lesson, applicability)
    now = utc_now()
    return {
        "id": "skill_smoke" + digest[:16],
        "status": "candidate",
        "shared_scope": {"organization_id": experience_eval.SCOPE["organization_id"], "team_id": experience_eval.SCOPE["team_id"]},
        "publisher_actor_id": publisher,
        "lesson": lesson,
        "applicability": applicability,
        "content_digest": digest,
        "sanitization_version": SKILL_SANITIZATION_VERSION,
        "version": 1,
        "parent_skill_id": None,
        "deprecation_reason": None,
        "created_at": now,
        "updated_at": now,
        "verified_at": None,
        "deprecated_at": None,
        "retrieved_count": 0,
    }


# --- population --------------------------------------------------------------------

@dataclass
class Population:
    args: argparse.Namespace
    protocol: PopulationProtocol
    mode: str
    root: Path
    launcher: Path
    harness: dict[str, Any]
    config: dict[str, Any]
    s_code_revision: str
    services: dict[str, Path]
    report: dict[str, Any] = field(default_factory=dict)
    skill: dict[str, Any] | None = None
    experience: dict[str, Any] | None = None
    receipts: list[dict[str, Any]] = field(default_factory=list)
    runs: list[dict[str, Any]] = field(default_factory=list)

    def service_directories(self, service: str) -> dict[str, Path]:
        return {name: self.services[service] / name for name in harness_run.SERVICE_DIRECTORIES}

    def daemon(self, service: str, actor: str, arm: str = "baseline", skill_id: str | None = None) -> ShopClient:
        environment, directories, _ = harness_run.prepare_service(self.root, self.config, self.services[service])
        environment.update(arm_environment(actor, arm, skill_id))
        return ShopClient(self.launcher, directories, environment, self.root / f"{service}.log", actor)

    # -- agent A: learn, evaluate, publish --

    def publisher_phase(self) -> None:
        root = self.root / PUBLISHER_ROOT
        root.mkdir()
        (root / experience_eval.RUNS).mkdir()
        services = {name: root / relative for name, relative in experience_eval.SERVICES.items()}
        for path in services.values():
            path.mkdir()
        evaluation = experience_eval.Evaluation(
            args=self.args, protocol=self.protocol.base, mode=self.mode, root=root, launcher=self.launcher,
            harness=self.harness, config=self.config, workspace_root=root / experience_eval.WORKSPACE_ROOT,
            services=services, promotion_mode="automatic",
        )
        evaluation.report = {"kind": experience_eval.REPORT_KIND, "mode": self.mode, "started_at": utc_now(), "status": "running", "promotion_mode": "automatic"}
        try:
            evaluation.seed()
            evaluation.snapshot_registry()
            evaluation.approve_experience()
            evaluation.run_probe()
            evaluation.run_matrix()
            texts = evaluation.experience_texts()
            poisoning = experience_eval.poisoning_assessment(
                probe_completed=evaluation.probe["completed"], probe_candidate=evaluation.probe_candidate,
                approved_ids_after_each_run=evaluation.approvals_after_runs, experience_id=evaluation.experience["id"],
                candidate_retrievals=evaluation.candidate_retrievals, experience_texts=texts,
            )
            references = ["runs/seed", "runs/probe", experience_eval.EXPERIENCE_FILE, experience_eval.SUBMISSION_FILE]
            references += sorted({f"runs/{task['track']}-{task['id']}" for task in self.protocol.held_out_tasks})
            submission = experience_eval.build_submission(
                experience=evaluation.experience, protocol=self.protocol.base, s_code_revision=self.s_code_revision,
                baseline=evaluation.outcomes("baseline"), candidate=evaluation.outcomes("candidate"),
                poisoning=poisoning, artifact_references=references,
            )
            gate = evaluation.submit(submission)
            evaluation.report.update(status="recorded", daemon_gate=gate, poisoning=poisoning, runs=evaluation.runs)
        except EvaluationError:
            evaluation.report["status"] = "aborted"
            evaluation.write_report()
            raise
        evaluation.write_report()
        self.experience = evaluation.experience
        self.report["publisher"] = {
            "actor": self.protocol.publisher_actor,
            "root": PUBLISHER_ROOT,
            "experience_id": evaluation.experience["id"],
            "experience_eligible": gate["eligible"],
            "experience_status": gate["experience_status"],
            "poisoning": poisoning["verdict"],
            "runs": len(evaluation.runs),
        }
        self.publish(gate)

    def publish(self, gate: dict[str, Any]) -> None:
        """Explicit publication from the registry profile, the only path from an experience to a skill."""

        experience = self.experience
        assert experience is not None
        with self.daemon("publisher-registry", self.protocol.publisher_actor) as api:
            status, body = self.publish_request(api, experience)
            publication: dict[str, Any] = {"attempted": True, "status": status, "smoke_synthesized": False}
            if status in (200, 201) and isinstance(body, dict):
                publication["skill_id"] = body["id"]
                self.skill = body
            elif self.mode == "smoke":
                # The smoke's evaluation is ineligible by construction, so the daemon refuses the
                # publication; the plumbing continues with a clearly labelled synthesized artifact
                # that the shop still re-sanitizes on import.
                publication.update(smoke_synthesized=True, refusal=body if isinstance(body, dict) else str(body))
                self.skill = smoke_skill_artifact(api_experience(api, experience["id"]), self.protocol.publisher_actor)
                publication["skill_id"] = self.skill["id"]
            else:
                raise EvaluationError(f"the daemon refused the publication with status {status}: {body}")
        (self.root / SKILL_FILE).write_text(json.dumps(self.skill, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        for absent in ("evidence", "source_experience_id", "workspace_key", "source_session_id", "source_turn_id"):
            if absent in self.skill:
                raise EvaluationError(f"the published skill exposes {absent}")
        self.report["publication"] = publication

    @staticmethod
    def publish_request(api: ShopClient, experience: dict[str, Any]) -> tuple[int, Any]:
        try:
            return api.publish_skill(experience["id"], experience["workspace_key"])
        except EvaluationError as error:
            message = str(error)
            for code in (409, 400, 404):
                if f"status {code}" in message:
                    return code, {"error": message}
            raise

    # -- shop --

    def shop_import(self) -> None:
        assert self.skill is not None
        with self.daemon(SHOP_SERVICE, self.protocol.publisher_actor) as api:
            report = api.import_skill(self.skill)
        if report["skill"]["status"] != "candidate":
            raise EvaluationError(f"the shop imported the skill as {report['skill']['status']}, not as a candidate")
        self.report["shop"] = {"service": SHOP_SERVICE, "imported": report["created"], "status": report["skill"]["status"], "transitions": []}

    # -- agents B and C --

    def run_task(self, actor: str, arm: str, task: dict[str, str], output: Path, service: str | None) -> dict[str, Any] | None:
        assert self.skill is not None
        command = [
            sys.executable, str(experience_eval.RUN_SCRIPT), "--track", task["track"], "--task", task["id"],
            "--s-code", str(self.launcher), "--output", str(output),
            "--model", self.protocol.base.model, "--permission-mode", self.protocol.base.permission_mode,
            "--timeout", str(self.args.timeout), "--grace-seconds", str(self.args.grace_seconds),
            "--grader-timeout", str(self.args.grader_timeout), "--workspace-root", str(self.root / f"workspace-{actor}"),
        ]
        if service is not None:
            command += ["--service-home", str(self.services[service])]
        if self.args.service_config:
            command += ["--service-config", self.args.service_config]
        if self.args.polyglot_root:
            command += ["--polyglot-root", self.args.polyglot_root]
        if self.args.playwright_browsers:
            command += ["--playwright-browsers", self.args.playwright_browsers]
        environment = {**os.environ, **arm_environment(actor, arm, self.skill["id"])}
        completed = subprocess.run(command, check=False, text=True, capture_output=True, env=environment, cwd=self.root)
        output.parent.mkdir(parents=True, exist_ok=True)
        (output.parent / f"{output.name}.runner.log").write_text(
            f"exit status: {completed.returncode}\n--- stdout ---\n{completed.stdout}\n--- stderr ---\n{completed.stderr}", encoding="utf-8"
        )
        record_path = output / harness_run.RECORD_NAME
        if not record_path.is_file():
            return None
        record = json.loads(record_path.read_text(encoding="utf-8"))
        if record.get("schema_version") != harness_run.SCHEMA_VERSION or record.get("kind") != harness_run.RECORD_KIND:
            raise EvaluationError(f"unexpected run record at {record_path}")
        return record

    def check_run(self, arm: str, record: dict[str, Any] | None, output: Path) -> list[str]:
        assert self.skill is not None
        if record is None:
            return []
        effective = record["turn"]["effective_model"]
        if effective is not None and effective != self.protocol.base.model:
            raise EvaluationError(f"{output.name} was served by model {effective}, the protocol declares {self.protocol.base.model}")
        if record["events"]["by_kind"].get("experience.retrieved"):
            raise EvaluationError(f"{output.name} retrieved local experiences; population arms run with experience memory off")
        retrieved = retrieved_skill_ids(output / harness_run.ARTIFACTS["events"], record["turn"]["session_id"], record["turn"]["turn_id"])
        if arm == "baseline":
            if retrieved or record["events"]["by_kind"].get("skill.retrieved"):
                raise EvaluationError(f"baseline run {output.name} retrieved shared skills; the baseline runs with the skill shop off")
        else:
            model_reached = record["usage"]["model_usage_events"] > 0 or record["turn"]["status"] == "completed"
            if model_reached and set(retrieved) != {self.skill["id"]}:
                raise EvaluationError(f"{arm} run {output.name} did not retrieve exactly the evaluated skill: {retrieved}")
        return retrieved

    def evaluator_phase(self, actor: str) -> None:
        assert self.skill is not None
        service = evaluator_service(actor)
        self.services[service].mkdir()
        with self.daemon(service, actor) as api:
            imported = api.import_skill(self.skill)
            if imported["skill"]["status"] != "candidate":
                raise EvaluationError(f"evaluator {actor} imported the skill as {imported['skill']['status']}")
        runs: list[dict[str, Any]] = []
        candidate_retrievals: list[list[str]] = []
        for entry in experience_eval.plan_matrix(self.protocol.base):
            name = experience_eval.run_name(entry)
            output = self.root / RUNS / actor / name
            record = self.run_task(actor, entry["arm"], entry["task"], output, service if entry["arm"] == "candidate" else None)
            retrieved = self.check_run(entry["arm"], record, output)
            if entry["arm"] == "candidate" and record is not None and retrieved:
                candidate_retrievals.append(retrieved)
            attempt = experience_eval.attempt_from_record(record, failure="the runner produced no record; see the runner log")
            run = {"agent": actor, "name": name, "arm": entry["arm"], "task": experience_eval.task_identity(entry["task"]), "repeat": entry["repeat"], "retrieved": retrieved, **attempt}
            runs.append(run)
            self.runs.append(run)
            print(json.dumps({"agent": actor, "run": name, "passed": attempt["passed"], "comparable": attempt["comparable"], "exclusion_reason": attempt["exclusion_reason"]}, sort_keys=True), flush=True)
        outcomes = {
            arm: [
                experience_eval.aggregate_outcome(task, [run for run in runs if run["arm"] == arm and run["task"]["id"] == task["id"] and run["task"]["track"] == task["track"]])
                for task in self.protocol.held_out_tasks
            ]
            for arm in ("baseline", "candidate")
        }
        safety = skill_safety_probe(skill_id=self.skill["id"], candidate_retrievals=candidate_retrievals, skill_texts=[self.skill["lesson"], self.skill["applicability"]])
        references = [f"runs/{actor}/{run['name']}" for run in runs] + [SKILL_FILE]
        receipt = build_receipt(
            actor=actor, skill=self.skill, protocol=self.protocol, s_code_revision=self.s_code_revision,
            baseline=outcomes["baseline"], candidate=outcomes["candidate"], safety=safety, artifact_references=references,
        )
        (self.root / RECEIPTS_DIR / f"{actor}.json").write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        accepted = self.submit_receipt(actor, receipt)
        self.receipts.append({"actor": actor, "safety": safety, "runs": len(runs), **accepted})

    def submit_receipt(self, actor: str, receipt: dict[str, Any]) -> dict[str, Any]:
        """POST the raw counts to the shop as the evaluator's identity, then verify what was stored."""

        assert self.skill is not None
        with self.daemon(SHOP_SERVICE, actor) as api:
            status, accepted = api.submit_receipt(self.skill["id"], receipt)
            if status != 201 or not isinstance(accepted, dict):
                raise EvaluationError(f"the shop answered the receipt with status {status}")
            stored = [item for item in api.list_receipts(self.skill["id"]) if item.get("id") == accepted["receipt"]["id"]]
            current = api.get_skill(self.skill["id"])
        if len(stored) != 1:
            raise EvaluationError("the submitted receipt is not listed by the shop")
        stored = stored[0]
        verdict = stored.get("verdict")
        if not isinstance(verdict, dict) or stored.get("evaluator_actor_id") != actor:
            raise EvaluationError("the shop stored no verdict or the wrong evaluator for the receipt")
        expected = {
            "baseline_attempts": sum(outcome["attempts"] for outcome in receipt["baseline"]),
            "baseline_passes": sum(outcome["passes"] for outcome in receipt["baseline"]),
            "candidate_attempts": sum(outcome["attempts"] for outcome in receipt["candidate"]),
            "candidate_passes": sum(outcome["passes"] for outcome in receipt["candidate"]),
        }
        mismatched = {key: (verdict.get(key), value) for key, value in expected.items() if verdict.get(key) != value}
        if mismatched:
            raise EvaluationError(f"the shop's stored counts differ from the submitted raw counts: {mismatched}")
        if stored.get("independent") is not True:
            raise EvaluationError(f"the shop did not record evaluator {actor} as independent of the publisher")
        if self.mode == "smoke" and stored.get("complete"):
            raise EvaluationError("a smoke receipt must never be complete")
        transition = accepted.get("transition")
        self.report["shop"]["transitions"].append({"after_receipt_of": actor, "transition": transition, "status": current["status"]})
        return {
            "receipt_id": stored["id"],
            "protocol_digest": stored["protocol_digest"],
            "complete": stored["complete"],
            "independent": stored["independent"],
            "safety_recorded": stored["safety"],
            "verdict": verdict,
            "transition": transition,
            "skill_status": current["status"],
        }

    # -- agent D --

    def consumer_phase(self) -> None:
        assert self.skill is not None
        actor = self.protocol.consumer_actor
        with self.daemon(SHOP_SERVICE, actor) as api:
            exported = api.get_skill(self.skill["id"])
            receipts = api.list_receipts(self.skill["id"])
        self.services[CONSUMER_SERVICE].mkdir()
        with self.daemon(CONSUMER_SERVICE, actor) as api:
            imported = api.import_skill(exported, receipts)
        status = imported["skill"]["status"]
        consumer: dict[str, Any] = {
            "actor": actor, "shop_status": exported["status"], "imported_status": status,
            "receipts_imported": imported["receipts_imported"], "status_recomputed_from_receipts": status == exported["status"],
        }
        if status == "verified":
            arm = "consumer"
        elif self.mode == "smoke":
            arm = "candidate"
            consumer["note"] = "smoke: the skill is not verified, the consumer turn uses the evaluation-only control"
        else:
            consumer["skipped"] = "the skill is not verified; a consumer never receives an unverified skill"
            self.report["consumer"] = consumer
            return
        task = self.protocol.held_out_tasks[0]
        output = self.root / RUNS / actor / f"consumer-{task['track']}-{task['id']}"
        record = self.run_task(actor, arm, task, output, CONSUMER_SERVICE)
        retrieved = self.check_run(arm, record, output)
        consumer.update(arm=arm, evaluation_only=arm != "consumer", retrieved=retrieved, task=experience_eval.task_identity(task), **experience_eval.attempt_from_record(record))
        self.report["consumer"] = consumer

    def write_report(self) -> None:
        self.report["finished_at"] = utc_now()
        (self.root / REPORT_FILE).write_text(json.dumps(self.report, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def api_experience(api: ShopClient, experience_id: str) -> dict[str, Any]:
    matches = [item for item in api.list_experiences() if item["id"] == experience_id]
    if len(matches) != 1:
        raise EvaluationError("the registry profile does not hold the evaluated experience")
    return matches[0]


def evaluate(args: argparse.Namespace) -> int:
    mode = args.mode
    # The caller's own shop settings never reach any agent or launcher call: the publisher runs
    # with the shop off and every evaluator arm sets its mode explicitly.
    os.environ[SKILL_SHOP_MODE_ENVIRONMENT] = "off"
    os.environ[SKILL_SHOP_SKILLS_ENVIRONMENT] = ""
    protocol = load_population_protocol(Path(args.protocol), mode)
    launcher = harness_run.resolve_binary(args.s_code)
    if mode == "dry-run":
        print_plan(protocol, mode, experience_eval.checkout_revision())
        return 0
    if not args.output:
        raise EvaluationError("--output is required for smoke and confirmatory evaluations")
    root = experience_eval.evaluation_root(args.output)
    config = harness_run.service_config(args.service_config)
    harness = harness_run.harness_identity(launcher)
    if mode == "confirmatory" and (harness["source_revision"] is None or harness["source_dirty"]):
        raise EvaluationError("a confirmatory evaluation needs a clean Git checkout so the S-Code revision can be attested")
    s_code_revision = harness["source_revision"] or experience_eval.UNKNOWN_REVISION
    if not (launcher.parent / experience_eval.DAEMON_BINARY).is_file():
        raise EvaluationError(f"{experience_eval.DAEMON_BINARY} must sit beside the launcher: {launcher.parent}")
    root.parent.mkdir(parents=True, exist_ok=True)
    root.mkdir()
    (root / PROTOCOL_FILE).write_text(json.dumps(protocol.document, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    (root / RUNS).mkdir()
    (root / RECEIPTS_DIR).mkdir()
    services = {
        "publisher-registry": root / PUBLISHER_ROOT / experience_eval.SERVICES["registry"],
        SHOP_SERVICE: root / SHOP_SERVICE,
        CONSUMER_SERVICE: root / CONSUMER_SERVICE,
    }
    for actor in protocol.evaluator_actors:
        services[evaluator_service(actor)] = root / evaluator_service(actor)
    services[SHOP_SERVICE].mkdir()
    population = Population(
        args=args, protocol=protocol, mode=mode, root=root, launcher=launcher, harness=harness, config=config,
        s_code_revision=s_code_revision, services=services,
    )
    population.report = {
        "schema_version": REPORT_SCHEMA_VERSION,
        "kind": REPORT_KIND,
        "mode": mode,
        "eligible_by_protocol": mode == "confirmatory",
        "note": (
            "plumbing smoke: fewer than five repeats, so no receipt is complete and no skill can be verified"
            if mode == "smoke" else "confirmatory protocol: exactly five repeats per evaluator arm"
        ),
        "started_at": utc_now(),
        "protocol": protocol.document,
        "harness": harness,
        "catalog_revision": protocol.base.catalog_revision,
        "shared_scope": {"organization_id": experience_eval.SCOPE["organization_id"], "team_id": experience_eval.SCOPE["team_id"]},
        "service_config": {"source": config["source"], "sha256": config["sha256"]},
        "status": "running",
    }
    try:
        population.publisher_phase()
        population.shop_import()
        for actor in protocol.evaluator_actors:
            population.evaluator_phase(actor)
        population.consumer_phase()
        population.report.update(skill=population.skill, receipts=population.receipts, runs=population.runs, status="recorded")
    except EvaluationError as error:
        population.report["status"] = "aborted"
        population.report["error"] = str(error)
        population.write_report()
        raise
    population.write_report()
    verified_by_gate = any(receipt["transition"] == "verified" for receipt in population.receipts)
    final_status = population.receipts[-1]["skill_status"] if population.receipts else (population.skill or {}).get("status")
    print(json.dumps({
        "report": str(root / REPORT_FILE),
        "mode": mode,
        "skill_id": population.skill["id"] if population.skill else None,
        "publication": population.report["publication"],
        "receipts": len(population.receipts),
        "independent_receipts": sum(1 for receipt in population.receipts if receipt["independent"]),
        "complete_receipts": sum(1 for receipt in population.receipts if receipt["complete"]),
        "skill_status": final_status,
        "verified_by_gate": verified_by_gate,
        "manual_verify_requested": False,
        "consumer": population.report.get("consumer", {}).get("retrieved"),
        "eligible_by_protocol": mode == "confirmatory",
    }, sort_keys=True))
    return 0 if mode == "smoke" or final_status == "verified" else 3


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description="Population skill-shop evaluator: publish, evaluate independently, verify deterministically, consume.")
    result.add_argument("--protocol", required=True, help="population protocol JSON file (validated completely before any run)")
    result.add_argument("--mode", required=True, choices=MODES, help="dry-run: validate and print the matrix; smoke: plumbing only; confirmatory: the fixed protocol")
    result.add_argument("--s-code", required=True, help="S-Code launcher; s-code-daemon must sit beside it")
    result.add_argument("--output", help="new evaluation directory beneath .work/ (smoke and confirmatory)")
    result.add_argument("--timeout", type=harness_run.bounded_int(1, 86_400), default=600, help="s-code exec --timeout seconds per run")
    result.add_argument("--grace-seconds", type=harness_run.bounded_int(0, 3_600), default=30)
    result.add_argument("--grader-timeout", type=float, default=120.0)
    result.add_argument("--service-config", help="caller configuration for the isolated services")
    result.add_argument("--service-settle-seconds", type=harness_run.bounded_int(0, 600), default=experience_eval.DEFAULT_SETTLE_SECONDS)
    result.add_argument("--polyglot-root", help="frozen polyglot checkout for algorithm tasks")
    result.add_argument("--playwright-browsers", help="browser cache passed to the frontend grader")
    return result


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        return evaluate(args)
    except EvaluationError as error:
        print(f"population skill evaluation error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
