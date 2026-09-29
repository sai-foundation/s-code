#!/usr/bin/env python3
"""Show how S-Code turns one agent's correction into a Skill another agent reuses.

Two modes, and they never blur into each other:

  replay   deterministic, offline, no provider. Narrates a recorded run that was
           reduced to its lifecycle and stripped of anything private.
  live     walks the same lifecycle against a running local daemon, through the
           product's own scoped endpoints. Every line is state this run read;
           there is no synthetic fallback, and a stage that cannot be shown stops
           the demo with what is missing instead of being simulated.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
import urllib.error
import urllib.parse
import urllib.request

ORDER = ("learned", "candidate", "evaluated", "published", "receipts", "verified", "reused", "refused")
TITLES = {
    "learned": "1 · LEARNED",
    "candidate": "2 · CANDIDATE",
    "evaluated": "3 · EVALUATED",
    "published": "4 · PUBLISHED",
    "receipts": "5 · RECEIPTS",
    "verified": "6 · VERIFIED",
    "reused": "7 · REUSED",
    "refused": "· REFUSED",
    "refined": "8 · REFINED",
}
RULE = "─" * 72


def render(states: list[dict], label: str) -> None:
    print(RULE)
    print(f"S-Code · self-evolution · {label}")
    print(RULE)
    for state in states:
        print()
        print(f"{TITLES.get(state['state'], state['state'].upper())}   {state['headline']}")
        for key, value in (state.get("evidence") or {}).items():
            if value is None or value == [] or value == "":
                continue
            if isinstance(value, list) and value and isinstance(value[0], dict):
                for item in value:
                    print(f"    {key}: " + ", ".join(f"{k}={v}" for k, v in item.items()))
                continue
            printed = " ".join(value) if isinstance(value, list) and all(isinstance(v, str) for v in value) else value
            print(f"    {key}: {printed}")
    print()
    print(RULE)
    print("The Skill another agent reused carries no evidence, no workspace key and no")
    print("source session: only a sanitized lesson, its applicability, and the receipts")
    print("that verified it. The poisoned candidate never left quarantine.")
    print(RULE)


def verify_fixture(document: dict) -> None:
    recorded = document.get("digest")
    body = {key: value for key, value in document.items() if key != "digest"}
    computed = hashlib.sha256(
        json.dumps(body, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    ).hexdigest()
    if recorded != computed:
        raise SystemExit(f"replay fixture digest does not match its contents: {recorded} != {computed}")
    states = [state["state"] for state in document["states"]]
    if states != list(ORDER):
        raise SystemExit(f"replay fixture states are out of order: {states}")


def replay(fixture: Path) -> int:
    document = json.loads(fixture.read_text(encoding="utf-8"))
    verify_fixture(document)
    render(document["states"], f"replay · recorded run · digest {document['digest'][:12]}")
    return 0


def connection() -> tuple[str, str]:
    """The local daemon's address and token, from the private connection file."""

    runtime = os.environ.get("S_CODE_RUNTIME_DIR")
    home = os.environ.get("S_CODE_HOME")
    candidates = [Path(runtime) / "daemon.json"] if runtime else []
    candidates += [Path(home) / "runtime" / "daemon.json"] if home else []
    candidates += [Path.home() / ".s-code" / "runtime" / "daemon.json"]
    for path in candidates:
        if path.is_file():
            document = json.loads(path.read_text(encoding="utf-8"))
            address = document.get("address") or document.get("url")
            token = document.get("token") or document.get("bearer")
            if address and token:
                return (address if address.startswith("http") else f"http://{address}"), token
    raise SystemExit(
        "live mode needs a running local daemon: start one with `s-code` and try again")


def call(base: str, token: str, path: str, method: str = "GET", body: dict | None = None):
    request = urllib.request.Request(
        f"{base}{path}", method=method,
        data=None if body is None else json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def scope_of(arguments: argparse.Namespace) -> dict[str, str]:
    """The organization, team and actor every scoped endpoint requires.

    There is no default: a demo that guessed a scope would read someone else's
    state or an empty one and call it a result.
    """

    scope = {
        "organization_id": arguments.organization or os.environ.get("S_CODE_ORGANIZATION_ID", ""),
        "team_id": arguments.team or os.environ.get("S_CODE_TEAM_ID", ""),
        "actor_id": arguments.actor or os.environ.get("S_CODE_ACTOR_ID", ""),
    }
    missing = sorted(key for key, value in scope.items() if not value)
    if missing:
        raise SystemExit(
            "live mode needs the scope its state belongs to: pass --organization, --team and "
            f"--actor (or set S_CODE_ORGANIZATION_ID, S_CODE_TEAM_ID, S_CODE_ACTOR_ID); "
            f"missing {', '.join(missing)}")
    return scope


def query(scope: dict[str, str], **extra: str) -> str:
    fields = {**scope, **{key: value for key, value in extra.items() if value}}
    return "?" + urllib.parse.urlencode(fields)


def read(base: str, token: str, path: str, label: str):
    """One scoped read, or a clear stop. Nothing is assumed when a call fails."""

    try:
        return call(base, token, path)
    except urllib.error.HTTPError as error:
        raise SystemExit(f"live mode could not read {label}: the daemon answered {error.code} "
                         f"{error.reason}") from None
    except (urllib.error.URLError, OSError) as error:
        raise SystemExit(f"live mode could not reach the local daemon for {label}: {error}") from None


def newest(items: list[dict]) -> dict | None:
    return sorted(items, key=lambda item: item.get("created_at") or "")[-1] if items else None


def live(arguments: argparse.Namespace) -> int:
    """Walk the real lifecycle against a running daemon.

    Every line is state this run just read from the product. Nothing is
    synthesized, and a stage that cannot be shown stops the demo with what is
    missing and how to produce it.
    """

    base, token = connection()
    scope = scope_of(arguments)
    health = read(base, token, "/v1/health", "the daemon's health")
    print(RULE)
    print("S-Code · self-evolution · live")
    print(RULE)
    print(f"    daemon: {health.get('version', 'unknown version')} · protocol "
          f"{health.get('protocol_version', 'unknown')}")
    print(f"    scope: {scope['organization_id']}/{scope['team_id']}/{scope['actor_id']}")

    quarantined = read(base, token, "/v1/experiences" + query(scope, status="candidate"),
                       "quarantined Experiences")
    approved = read(base, token, "/v1/experiences" + query(scope, status="approved"),
                    "approved Experiences")
    print()
    print(f"{TITLES['learned']}   Experiences this daemon holds right now")
    print(f"    approved: {len(approved)}    candidate (quarantined): {len(quarantined)}")
    if not approved:
        raise SystemExit(
            "live mode found no approved Experience in this scope. Run a corrective episode, "
            "approve its candidate, and try again; nothing here is filled in for you.")
    for item in approved[:3]:
        print()
        print(f"{TITLES['candidate']}   approved Experience {item['id']}")
        print(f"    lesson: {item.get('lesson', '')[:72]}")
        print(f"    model: {item.get('model', 'unknown')} · retrieved {item.get('retrieved_count', 0)}x")

    skills = read(base, token, "/v1/skills" + query(scope), "shared Skills")
    verified = [skill for skill in skills if skill.get("status") == "verified"]
    print()
    print(f"{TITLES['published']}   Skills this scope can see: {len(skills)}"
          f" (candidate {sum(1 for s in skills if s.get('status') == 'candidate')},"
          f" verified {len(verified)},"
          f" deprecated {sum(1 for s in skills if s.get('status') == 'deprecated')})")
    if not verified:
        raise SystemExit(
            "live mode found no verified Skill in this scope. Publish an approved Experience and "
            "collect the receipts the gate needs from independent evaluators, then try again; the "
            "demo will not pretend a Skill was verified.")
    skill = newest(verified)
    print()
    print(f"{TITLES['evaluated']}   verified Skill {skill['id']}")
    print(f"    lesson: {skill.get('lesson', '')[:72]}")
    print(f"    applicability: {skill.get('applicability', '')[:64]}")
    print(f"    content digest: {str(skill.get('content_digest', ''))[:16]}")

    receipts = read(base, token, f"/v1/skills/{skill['id']}/evaluations" + query(scope),
                    "the Skill's receipts")
    print()
    print(f"{TITLES['receipts']}   receipts on record: {len(receipts)}")
    for receipt in receipts[:4]:
        print(f"    {receipt.get('evaluator_actor_id')} · independent="
              f"{receipt.get('independent')} origin={receipt.get('origin')} "
              f"safety={receipt.get('safety')}")
    independent = {receipt.get("evaluator_actor_id") for receipt in receipts
                   if receipt.get("independent") and receipt.get("origin") == "direct"}
    if len(independent) < 2:
        raise SystemExit(
            f"live mode found {len(independent)} independent evaluator(s) behind "
            f"{skill['id']}: the verification gate needs two, and only receipts filed by their own "
            "evaluator count. Nothing is inferred from an imported receipt.")
    print()
    print(f"{TITLES['verified']}   verified by {len(independent)} independent evaluators, "
          f"at {skill.get('verified_at', 'unknown time')}")

    reuse = int(skill.get("retrieved_count") or 0)
    print()
    print(f"{TITLES['reused']}   times this Skill has been injected into a turn: {reuse}")
    if reuse == 0:
        raise SystemExit(
            f"live mode found no reuse of {skill['id']} yet. Run a turn in an actor that requests "
            "it with the shop enabled, then try again; the demo counts only real retrievals.")

    print()
    print(f"{TITLES['refused']}   candidates still in quarantine: {len(quarantined)}")
    if quarantined:
        for item in quarantined[:3]:
            print(f"    {item['id']} · never approved, never published · "
                  f"{item.get('lesson', '')[:48]}")
    else:
        print("    none right now; the replay run shows a poisoned candidate held in quarantine.")

    registry = arguments.registry or os.environ.get("S_CODE_SKILL_REGISTRY_URL", "")
    registry_token = arguments.registry_token or os.environ.get("S_CODE_SKILL_REGISTRY_TOKEN", "")
    print()
    if registry and registry_token:
        lineage = read(registry.rstrip("/"), registry_token,
                       f"/v1/skills/{skill['id']}/lineage", "the Skill's lineage")
        active = lineage.get("active_id") or lineage.get("requested_id")
        print(f"{TITLES['refined']}   lineage in the registry: requested "
              f"{lineage.get('requested_id')} · active {active}")
        for version in (lineage.get("versions") or [])[:4]:
            print(f"    version {version.get('version')} · {version.get('id')} · "
                  f"{version.get('status')}")
    else:
        print(f"{TITLES['refined']}   not shown: collaborative evolution lives in the shared "
              "registry.")
        print("    pass --registry and --registry-token (or set S_CODE_SKILL_REGISTRY_URL and")
        print("    S_CODE_SKILL_REGISTRY_TOKEN) to read the lineage this Skill belongs to.")
    print()
    print(RULE)
    print("Every line above was read from this daemon in this run. A stage that could not")
    print("be shown stopped the demo instead of being simulated.")
    print(RULE)
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("replay", "live"), default="replay")
    parser.add_argument("--fixture", default=str(Path(__file__).parent / "fixtures" / "self-evolve-demo.json"))
    parser.add_argument("--organization", default="")
    parser.add_argument("--team", default="")
    parser.add_argument("--actor", default="")
    parser.add_argument("--registry", default="")
    parser.add_argument("--registry-token", dest="registry_token", default="")
    arguments = parser.parse_args(argv)
    if arguments.mode == "replay":
        return replay(Path(arguments.fixture))
    return live(arguments)


if __name__ == "__main__":
    sys.exit(main())
