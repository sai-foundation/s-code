#!/usr/bin/env python3
"""Show how S-Code turns one agent's correction into a Skill another agent reuses.

Two modes, and they never blur into each other:

  replay   deterministic, offline, no provider. Narrates a recorded run that was
           reduced to its lifecycle and stripped of anything private.
  live     walks the same lifecycle against a running local daemon. Every line is
           something that just happened; there is no synthetic fallback, and a
           step that cannot run stops the demo instead of being simulated.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
import urllib.error
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


def call(base: str, token: str, path: str, method: str = "GET", body: dict | None = None) -> dict:
    request = urllib.request.Request(
        f"{base}{path}", method=method,
        data=None if body is None else json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


def live() -> int:
    """Walk the real lifecycle. Anything that cannot happen stops the demo."""

    base, token = connection()
    try:
        health = call(base, token, "/v1/health")
    except (urllib.error.URLError, OSError) as error:
        raise SystemExit(f"live mode could not reach the local daemon: {error}")
    print(RULE)
    print("S-Code · self-evolution · live")
    print(RULE)
    print(f"    daemon: {health.get('version', 'unknown version')}")
    experiences = call(base, token, "/v1/experiences?status=approved").get("items", [])
    candidates = call(base, token, "/v1/experiences?status=candidate").get("items", [])
    print()
    print(f"{TITLES['candidate']}   experiences this daemon holds right now")
    print(f"    approved: {len(experiences)}    candidate (quarantined): {len(candidates)}")
    for item in candidates[:3]:
        print(f"    quarantined: {item['id']} · {item.get('lesson', '')[:60]}")
    if not experiences:
        raise SystemExit(
            "live mode found no approved Experience yet. Run a corrective episode first, "
            "approve it, and try again; nothing here is simulated.")
    for item in experiences[:3]:
        print()
        print(f"{TITLES['evaluated']}   approved Experience {item['id']}")
        print(f"    lesson: {item.get('lesson', '')[:72]}")
    skills = call(base, token, "/v1/skills").get("items", [])
    verified = [skill for skill in skills if skill.get("status") == "verified"]
    print()
    print(f"{TITLES['published']}   skills this daemon can see: {len(skills)}")
    print(f"{TITLES['verified']}   verified by receipts: {len(verified)}")
    for skill in verified[:3]:
        print(f"    {skill['id']} · {skill.get('lesson', '')[:60]}")
    if not verified:
        print("    none yet: publish an approved Experience and collect two independent")
        print("    receipts to see a Skill reach verified. Nothing is filled in for you.")
    print()
    print(RULE)
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("replay", "live"), default="replay")
    parser.add_argument("--fixture", default=str(Path(__file__).parent / "fixtures" / "self-evolve-demo.json"))
    arguments = parser.parse_args(argv)
    if arguments.mode == "replay":
        return replay(Path(arguments.fixture))
    return live()


if __name__ == "__main__":
    sys.exit(main())
