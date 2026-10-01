#!/usr/bin/env python3
"""Turn one real self-evolution run into a public-safe replay fixture.

A recorded run is full of things that must not ship: absolute cluster paths,
provider endpoints, registry tokens, machine names and the identifiers of a
private evaluation. This reduces such a run to the lifecycle it demonstrates -
which states happened, in what order, with what evidence - and rewrites every
identifier into a stable demo name. The result carries its own digest so a
reviewer can see it was generated rather than hand-written.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys
from typing import Any

SCHEMA_VERSION = 1
# Identifier shapes the daemon and registry mint. Each is replaced by a stable
# demo name so the fixture reads well and reveals nothing about a real run.
IDENTIFIER = re.compile(r"\b(?:exp|skill|ses|turn|principal|eval|evt|item|tool|call)_[A-Za-z0-9]+\b")
# Anything that could carry a path, a host or a secret is never copied.
FORBIDDEN = re.compile(
    r"(/network/|/home/|/tmp/|127\.0\.0\.1|localhost|openrouter|api\.openai|"
    r"Bearer\s|sk-[A-Za-z0-9]|token|password|apikey|api_key)", re.IGNORECASE)


class Sanitiser:
    """Stable demo names for the identifiers of one recorded run."""

    def __init__(self) -> None:
        self.names: dict[str, str] = {}
        self.counts: dict[str, int] = {}

    def name_for(self, identifier: str) -> str:
        if identifier not in self.names:
            kind = identifier.split("_", 1)[0]
            self.counts[kind] = self.counts.get(kind, 0) + 1
            self.names[identifier] = f"{kind}_demo{self.counts[kind]:02d}"
        return self.names[identifier]

    def text(self, value: str) -> str:
        return IDENTIFIER.sub(lambda match: self.name_for(match.group(0)), value)


def read(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def lifecycle(run: Path, sanitiser: Sanitiser) -> list[dict[str, Any]]:
    """The six states of the mechanism plus the refusal, in the order they happened."""

    population = read(run / "report.json")
    publisher = read(run / "publisher" / "report.json")
    experience = read(run / "publisher" / "experience.json")
    evidence = experience.get("evidence") or {}
    distillation = evidence.get("distillation") or {}
    acquisition = publisher.get("source_acquisition") or {}
    gate = publisher.get("daemon_gate") or {}
    poisoning = publisher.get("poisoning") or {}
    skill = population.get("skill") or {}
    receipts = population.get("receipts") or []
    consumer = population.get("consumer") or {}
    shop = population.get("shop") or {}

    states = [
        {
            "state": "learned",
            "headline": "An episode failed its own verifier, was fixed, and passed it again",
            "evidence": {
                "verifier": evidence.get("verifier"),
                "failed_attempts": evidence.get("failed_attempts"),
                "edited_paths": evidence.get("edited_paths"),
                "attempts_used": acquisition.get("attempts_used"),
                "model_calls": (acquisition.get("attempts") or [{}])[0].get("usage", {}).get("model_calls"),
                "tool_calls": (acquisition.get("attempts") or [{}])[0].get("usage", {}).get("tool_calls"),
            },
        },
        {
            "state": "candidate",
            "headline": "The recovery was distilled into an Experience, quarantined until evaluated",
            "evidence": {
                "experience": sanitiser.name_for(experience["id"]),
                "status_at_creation": "candidate",
                "lesson": sanitiser.text(experience.get("lesson", "")),
                "applicability": sanitiser.text(distillation.get("applicability", "")),
                "distillation": distillation.get("status"),
            },
        },
        {
            "state": "evaluated",
            "headline": "Its owner evaluated it on held-out tasks and approved it",
            "evidence": {
                "eligible": gate.get("eligible"),
                "experience_status": gate.get("experience_status"),
                "counts_verified": gate.get("counts_verified"),
            },
        },
        {
            "state": "published",
            "headline": "The approved Experience was published as a sanitized Skill",
            "evidence": {
                "skill": sanitiser.name_for(skill.get("id", "skill_unknown")),
                "status_at_publication": "candidate",
                "publication_status": (population.get("publication") or {}).get("status"),
                "synthesized": (population.get("publication") or {}).get("smoke_synthesized"),
                "lesson": sanitiser.text(skill.get("lesson", "")),
            },
        },
        {
            "state": "receipts",
            "headline": "Two independent evaluators ran the held-out task and filed receipts",
            "evidence": {
                "receipts": [
                    {
                        "evaluator": receipt.get("actor"),
                        "independent": receipt.get("independent"),
                        "authoritative": receipt.get("authoritative"),
                        "complete": receipt.get("complete"),
                        "safety": (receipt.get("safety") or {}).get("verdict"),
                        "transition": receipt.get("transition"),
                    }
                    for receipt in receipts
                ],
                "gate_transitions": shop.get("transitions"),
            },
        },
        {
            "state": "verified",
            "headline": "The registry's deterministic gate promoted the Skill to verified",
            "evidence": {
                "skill_status": (receipts[-1].get("skill_status") if receipts
                                 else population.get("skill_status") or skill.get("status")),
                "verified_by_gate": any(r.get("transition") == "verified" for r in receipts),
                "independent_receipts": sum(1 for r in receipts if r.get("independent")),
            },
        },
        {
            "state": "reused",
            "headline": "A fresh agent that never saw the source task retrieved that Skill and solved the task",
            "evidence": {
                "consumer": consumer.get("actor"),
                "retrieved": [sanitiser.name_for(item) for item in (consumer.get("retrieved") or [])],
                "passed": consumer.get("passed"),
                "independent_evaluators": consumer.get("independent_evaluators"),
            },
        },
        {
            "state": "refused",
            "headline": "A candidate distilled from poisoned text stayed quarantined for ever",
            "evidence": {
                "probe_task": (poisoning.get("probe_task") or {}).get("id"),
                "verdict": poisoning.get("verdict"),
                "candidate_remained_unapproved": poisoning.get("candidate_remained_unapproved"),
                "harmful_rule_absent_from_requests": poisoning.get("harmful_rule_absent_from_requests"),
            },
        },
    ]
    return states


def scrub(value: Any, sanitiser: Sanitiser) -> Any:
    """Rewrite identifiers everywhere and refuse anything that could leak."""

    if isinstance(value, dict):
        return {key: scrub(item, sanitiser) for key, item in value.items()}
    if isinstance(value, list):
        return [scrub(item, sanitiser) for item in value]
    if isinstance(value, str):
        cleaned = sanitiser.text(value)
        if FORBIDDEN.search(cleaned):
            raise ValueError(f"refusing to ship text that may carry a path, host or secret: {cleaned[:80]!r}")
        return cleaned
    return value


def build(run: Path) -> dict[str, Any]:
    sanitiser = Sanitiser()
    states = scrub(lifecycle(run, sanitiser), sanitiser)
    document = {
        "schema_version": SCHEMA_VERSION,
        "kind": "s-code-self-evolution-demo",
        "note": "Recorded from a real run, then reduced to the lifecycle and renamed. "
                "No paths, hosts, tokens or private identifiers are included.",
        "states": states,
    }
    body = json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    document["digest"] = hashlib.sha256(body.encode("utf-8")).hexdigest()
    return document


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run", required=True, help="a finished population evaluation directory")
    parser.add_argument("--out", required=True, help="where to write the public-safe fixture")
    arguments = parser.parse_args(argv)
    document = build(Path(arguments.run))
    Path(arguments.out).write_text(json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {arguments.out} with {len(document['states'])} states, digest {document['digest'][:12]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
