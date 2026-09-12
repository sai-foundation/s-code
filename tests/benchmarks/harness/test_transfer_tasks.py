#!/usr/bin/env python3
"""Construction audit and grader validation for the experience-transfer task families.

Each family pairs one source task, from which an experience is learned, with
a held-out task that shares a procedural skill but not its answer. These
tests are a benchmark-construction audit, not a proof of non-leakage: they
check the pairing is structurally distinct, that starter packages carry no
implementation, that graders accept a known-good solution and reject a
deliberately wrong one, and that the catalog metadata stays consistent.
"""

from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
TOOL = ROOT / "tests/test-harness-benchmark.py"
WORK_ROOT = ROOT / ".work"
spec = importlib.util.spec_from_file_location("harness_benchmark", TOOL)
benchmark = importlib.util.module_from_spec(spec)
spec.loader.exec_module(benchmark)

# Task-specific vocabulary that must not cross between a source task and its
# held-out partner: package and command names, file names, flags, output keys
# and fixture literals. Generic engineering words are deliberately absent.
SPECIFICS = {
    "logline-normalizer": ["logline", "EVENTS.jsonl", "--output", "scheduler", "queue depth", "cannot read input", "worker-2", "retry budget", "component"],
    "service-config-checker": ["confcheck", "SUMMARY.json", "--summary", "request_timeout_seconds", "max_body_kib", "billing-api", "[flags]", "configparser"],
    "checkpoint-registry": ["checkpoints", "REGISTRY.json", "--metrics", "--step", "warmup", "epoch-1", "latest"],
    "ledger-compactor": ["ledger", "JOURNAL.csv", "BALANCES.json", "--snapshot", "through_seq", "folded", "alice", "delta"],
}

# Known-good solutions used only to validate the graders; they never enter a
# fixture. Candidates cannot import sys or raise SystemExit, so errors are
# reported through the argument parser's exit.
REFERENCE = {
    "logline-normalizer": {
        "logline/__init__.py": '''"""Logline normalizer."""
from __future__ import annotations

import re

LEVELS = {"debug", "info", "warn", "error"}
TIMESTAMP = re.compile(r"^\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}Z$")
COMPONENT = re.compile(r"^[a-z][a-z0-9_-]*$")


class LineError(ValueError):
    def __init__(self, number: int, reason: str) -> None:
        super().__init__(f"line {number}: {reason}")


def parse_line(number: int, line: str) -> dict:
    parts = line.split(" ", 2)
    if len(parts) != 3:
        raise LineError(number, "expected timestamp, level and component")
    timestamp, level, rest = parts
    if not TIMESTAMP.match(timestamp):
        raise LineError(number, "invalid timestamp")
    level = level.lower()
    if level not in LEVELS:
        raise LineError(number, "unknown level")
    component, separator, message = rest.partition(": ")
    if not separator:
        if not rest.endswith(":"):
            raise LineError(number, "expected component followed by a colon")
        component, message = rest[:-1], ""
    if not COMPONENT.match(component):
        raise LineError(number, "invalid component")
    message = message.strip()
    if not message:
        raise LineError(number, "empty message")
    return {"timestamp": timestamp, "level": level, "component": component, "message": message}


def parse_log(text: str) -> list[dict]:
    events = []
    for number, line in enumerate(text.splitlines(), start=1):
        if line.strip():
            events.append(parse_line(number, line.strip()))
    return events
''',
        "logline/__main__.py": '''from __future__ import annotations

import argparse
import json
from pathlib import Path

from logline import LineError, parse_log


def main() -> None:
    parser = argparse.ArgumentParser(prog="python3 -m logline")
    parser.add_argument("input")
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    try:
        text = Path(args.input).read_text(encoding="utf-8")
    except OSError:
        parser.exit(2, f"{args.input}: cannot read input\\n")
    try:
        events = parse_log(text)
    except LineError as error:
        parser.exit(2, f"{error}\\n")
    Path(args.output).write_text("".join(json.dumps(event, sort_keys=True) + "\\n" for event in events), encoding="utf-8")
    print(json.dumps({"events": len(events)}))


main()
''',
    },
    "service-config-checker": {
        "confcheck/__init__.py": '''"""Service config checker."""
from __future__ import annotations

import configparser
import re

NAME = re.compile(r"^[a-z][a-z0-9-]*$")
SECTIONS = {"service": ("name", "port", "workers"), "limits": ("request_timeout_seconds", "max_body_kib"), "flags": ()}


class ConfigError(ValueError):
    pass


def integer(section: str, key: str, value: str, low: int, high: int) -> int:
    try:
        number = int(value)
    except ValueError:
        raise ConfigError(f"[{section}] {key}: expected an integer") from None
    if not low <= number <= high:
        raise ConfigError(f"[{section}] {key}: expected {low} to {high}")
    return number


def decimal(section: str, key: str, value: str, high: float) -> float:
    try:
        number = float(value)
    except ValueError:
        raise ConfigError(f"[{section}] {key}: expected a number") from None
    if not (0 < number <= high):
        raise ConfigError(f"[{section}] {key}: expected more than 0 and at most {high:g}")
    return number


def validate(text: str) -> dict:
    parser = configparser.ConfigParser(interpolation=None)
    try:
        parser.read_string(text)
    except configparser.Error as error:
        raise ConfigError(str(error).splitlines()[0]) from None
    for section in SECTIONS:
        if not parser.has_section(section):
            raise ConfigError(f"[{section}]: missing section")
    for section in parser.sections():
        if section not in SECTIONS:
            raise ConfigError(f"[{section}]: unknown section")
    for section, keys in SECTIONS.items():
        if not keys:
            continue
        for key in parser.options(section):
            if key not in keys:
                raise ConfigError(f"[{section}] {key}: unknown key")
        for key in keys:
            if key not in parser[section]:
                raise ConfigError(f"[{section}] {key}: missing key")
    service, limits = parser["service"], parser["limits"]
    name = service["name"].strip()
    if not NAME.match(name):
        raise ConfigError("[service] name: expected [a-z][a-z0-9-]*")
    summary = {
        "service": {"name": name, "port": integer("service", "port", service["port"], 1024, 65535), "workers": integer("service", "workers", service["workers"], 1, 64)},
        "limits": {"request_timeout_seconds": decimal("limits", "request_timeout_seconds", limits["request_timeout_seconds"], 300), "max_body_kib": integer("limits", "max_body_kib", limits["max_body_kib"], 1, 65536)},
        "flags": {},
    }
    for key, value in parser.items("flags"):
        if value not in ("on", "off"):
            raise ConfigError(f"[flags] {key}: expected on or off")
        summary["flags"][key] = value == "on"
    return summary
''',
        "confcheck/__main__.py": '''from __future__ import annotations

import argparse
import json
from pathlib import Path

from confcheck import ConfigError, validate


def main() -> None:
    parser = argparse.ArgumentParser(prog="python3 -m confcheck")
    parser.add_argument("config")
    parser.add_argument("--summary", required=True)
    args = parser.parse_args()
    try:
        text = Path(args.config).read_text(encoding="utf-8")
    except OSError:
        parser.exit(2, f"{args.config}: cannot read configuration\\n")
    try:
        summary = validate(text)
    except ConfigError as error:
        parser.exit(2, f"{args.config}: {error}\\n")
    Path(args.summary).write_text(json.dumps(summary, indent=2, sort_keys=True) + "\\n", encoding="utf-8")
    print(json.dumps({"flags": len(summary["flags"])}))


main()
''',
    },
    "checkpoint-registry": {
        "checkpoints/__init__.py": '''"""Checkpoint registry."""
from __future__ import annotations

import json
import math
import os
from pathlib import Path
import re
import tempfile

NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]*$")


class RegistryError(ValueError):
    pass


def empty() -> dict:
    return {"checkpoints": [], "latest": None}


def load_registry(path: Path) -> dict:
    if not path.exists():
        return empty()
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        raise RegistryError(f"{path}: registry is not valid JSON") from None
    shape = RegistryError(f"{path}: registry has the wrong shape")
    if not isinstance(data, dict) or set(data) != {"checkpoints", "latest"} or not isinstance(data["checkpoints"], list):
        raise shape
    for entry in data["checkpoints"]:
        if not isinstance(entry, dict) or set(entry) != {"metrics", "name", "step"}:
            raise shape
        if not isinstance(entry["name"], str) or isinstance(entry["step"], bool) or not isinstance(entry["step"], int) or not isinstance(entry["metrics"], dict):
            raise shape
    names = [entry["name"] for entry in data["checkpoints"]]
    if data["latest"] != (names[-1] if names else None):
        raise shape
    return data


def parse_metrics(text: str) -> dict:
    try:
        metrics = json.loads(text)
    except ValueError:
        raise RegistryError("metrics: not valid JSON") from None
    if not isinstance(metrics, dict):
        raise RegistryError("metrics: expected a JSON object")
    for key, value in metrics.items():
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
            raise RegistryError(f"metrics: {key} must be a finite number")
    return metrics


def write_atomically(path: Path, text: str) -> None:
    handle, temporary = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=path.parent)
    try:
        with os.fdopen(handle, "w", encoding="utf-8") as sink:
            sink.write(text)
            sink.flush()
            os.fsync(sink.fileno())
        os.replace(temporary, path)
    except BaseException:
        try:
            os.unlink(temporary)
        except OSError:
            pass
        raise


def record(path: Path, name: str, step: int, metrics_text: str) -> dict:
    registry = load_registry(path)
    if not NAME.match(name):
        raise RegistryError("name: invalid checkpoint name")
    if any(entry["name"] == name for entry in registry["checkpoints"]):
        raise RegistryError("name: already recorded")
    if step < 0 or any(entry["step"] >= step for entry in registry["checkpoints"]):
        raise RegistryError("step: must be greater than every recorded step")
    metrics = parse_metrics(metrics_text)
    registry["checkpoints"].append({"metrics": metrics, "name": name, "step": step})
    registry["latest"] = name
    write_atomically(path, json.dumps(registry, indent=2, sort_keys=True) + "\\n")
    return registry
''',
        "checkpoints/__main__.py": '''from __future__ import annotations

import argparse
import json
from pathlib import Path

from checkpoints import RegistryError, load_registry, record


def main() -> None:
    parser = argparse.ArgumentParser(prog="python3 -m checkpoints")
    parser.add_argument("registry")
    commands = parser.add_subparsers(dest="command", required=True)
    recorder = commands.add_parser("record")
    recorder.add_argument("name")
    recorder.add_argument("--step", type=int, required=True)
    recorder.add_argument("--metrics", required=True)
    commands.add_parser("latest")
    args = parser.parse_args()
    path = Path(args.registry)
    try:
        if args.command == "record":
            registry = record(path, args.name, args.step, args.metrics)
            print(json.dumps({"count": len(registry["checkpoints"]), "latest": registry["latest"]}))
        else:
            registry = load_registry(path)
            entries = registry["checkpoints"]
            print(json.dumps(entries[-1] if entries else None, sort_keys=True, separators=(",", ":")))
    except RegistryError as error:
        parser.exit(2, f"{error}\\n")


main()
''',
    },
    "ledger-compactor": {
        "ledger/__init__.py": '''"""Ledger compactor."""
from __future__ import annotations

import json
import os
from pathlib import Path
import re
import tempfile

ACCOUNT = re.compile(r"^[a-z][a-z0-9_]*$")
DELTA = re.compile(r"^[+-]?\\d+$")


class LedgerError(ValueError):
    pass


def load_snapshot(path: Path) -> dict:
    if not path.exists():
        return {"accounts": {}, "through_seq": 0}
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        raise LedgerError(f"{path}: snapshot is not valid JSON") from None
    shape = LedgerError(f"{path}: snapshot has the wrong shape")
    if not isinstance(data, dict) or set(data) != {"accounts", "through_seq"} or not isinstance(data["accounts"], dict):
        raise shape
    if isinstance(data["through_seq"], bool) or not isinstance(data["through_seq"], int) or data["through_seq"] < 0:
        raise shape
    for account, balance in data["accounts"].items():
        if not ACCOUNT.match(account) or isinstance(balance, bool) or not isinstance(balance, int):
            raise shape
    return data


def parse_journal(text: str, through_seq: int) -> list[tuple[int, str, int]]:
    entries = []
    last = through_seq
    for number, line in enumerate(text.splitlines(), start=1):
        if not line.strip():
            raise LedgerError(f"line {number}: blank line")
        parts = line.split(",")
        if len(parts) != 3:
            raise LedgerError(f"line {number}: expected seq,account,delta")
        seq_text, account, delta_text = parts
        if not seq_text.isdigit() or int(seq_text) <= 0:
            raise LedgerError(f"line {number}: seq must be a positive integer")
        seq = int(seq_text)
        if seq <= last:
            raise LedgerError(f"line {number}: seq {seq} is not after {last}")
        if not ACCOUNT.match(account):
            raise LedgerError(f"line {number}: invalid account")
        if not DELTA.match(delta_text):
            raise LedgerError(f"line {number}: delta must be a signed integer")
        entries.append((seq, account, int(delta_text)))
        last = seq
    return entries


def fold(snapshot: dict, entries: list[tuple[int, str, int]]) -> dict:
    accounts = dict(snapshot["accounts"])
    for _, account, delta in entries:
        accounts[account] = accounts.get(account, 0) + delta
    return {"accounts": {name: balance for name, balance in sorted(accounts.items()) if balance != 0}, "through_seq": entries[-1][0]}


def write_atomically(path: Path, text: str) -> None:
    handle, temporary = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=path.parent)
    try:
        with os.fdopen(handle, "w", encoding="utf-8") as sink:
            sink.write(text)
            sink.flush()
            os.fsync(sink.fileno())
        os.replace(temporary, path)
    except BaseException:
        try:
            os.unlink(temporary)
        except OSError:
            pass
        raise


def compact(journal: Path, snapshot_path: Path) -> dict:
    snapshot = load_snapshot(snapshot_path)
    try:
        text = journal.read_text(encoding="utf-8")
    except OSError:
        raise LedgerError(f"{journal}: cannot read journal") from None
    entries = parse_journal(text, snapshot["through_seq"])
    if not entries:
        return {"folded": 0, "through_seq": snapshot["through_seq"]}
    folded = fold(snapshot, entries)
    write_atomically(snapshot_path, json.dumps(folded, indent=2, sort_keys=True) + "\\n")
    write_atomically(journal, "")
    return {"folded": len(entries), "through_seq": folded["through_seq"]}
''',
        "ledger/__main__.py": '''from __future__ import annotations

import argparse
import json
from pathlib import Path

from ledger import LedgerError, compact


def main() -> None:
    parser = argparse.ArgumentParser(prog="python3 -m ledger")
    commands = parser.add_subparsers(dest="command", required=True)
    compactor = commands.add_parser("compact")
    compactor.add_argument("journal")
    compactor.add_argument("--snapshot", required=True)
    args = parser.parse_args()
    try:
        print(json.dumps(compact(Path(args.journal), Path(args.snapshot)), sort_keys=True))
    except LedgerError as error:
        parser.exit(2, f"{error}\\n")


main()
''',
    },
}

# One deliberately wrong solution per task: the primary behaviour works but
# the transferable procedural property is violated, so the grader must reject it.
WRONG = {
    "logline-normalizer": {
        "logline/__init__.py": REFERENCE["logline-normalizer"]["logline/__init__.py"],
        # Writes events while parsing: an existing output is clobbered before a later line fails.
        "logline/__main__.py": REFERENCE["logline-normalizer"]["logline/__main__.py"].replace(
            '''    try:
        events = parse_log(text)
    except LineError as error:
        parser.exit(2, f"{error}\\n")
    Path(args.output).write_text("".join(json.dumps(event, sort_keys=True) + "\\n" for event in events), encoding="utf-8")
''',
            '''    from logline import parse_line
    events = []
    with Path(args.output).open("w", encoding="utf-8") as sink:
        for number, line in enumerate(text.splitlines(), start=1):
            if not line.strip():
                continue
            try:
                event = parse_line(number, line.strip())
            except LineError as error:
                parser.exit(2, f"{error}\\n")
            events.append(event)
            sink.write(json.dumps(event, sort_keys=True) + "\\n")
''',
        ),
    },
    "service-config-checker": {
        # Validates the flags only after the summary was written.
        "confcheck/__init__.py": REFERENCE["service-config-checker"]["confcheck/__init__.py"].replace(
            '''    for key, value in parser.items("flags"):
        if value not in ("on", "off"):
            raise ConfigError(f"[flags] {key}: expected on or off")
        summary["flags"][key] = value == "on"
    return summary
''',
            '''    summary["flags"] = {key: value == "on" for key, value in parser.items("flags")}
    summary["_invalid_flags"] = [key for key, value in parser.items("flags") if value not in ("on", "off")]
    return summary
''',
        ),
        "confcheck/__main__.py": REFERENCE["service-config-checker"]["confcheck/__main__.py"].replace(
            '''    Path(args.summary).write_text(json.dumps(summary, indent=2, sort_keys=True) + "\\n", encoding="utf-8")
''',
            '''    invalid = summary.pop("_invalid_flags")
    Path(args.summary).write_text(json.dumps(summary, indent=2, sort_keys=True) + "\\n", encoding="utf-8")
    if invalid:
        parser.exit(2, f"{args.config}: [flags] {invalid[0]}: expected on or off\\n")
''',
        ),
    },
    "checkpoint-registry": {
        # Treats an unreadable registry as empty and overwrites it.
        "checkpoints/__init__.py": REFERENCE["checkpoint-registry"]["checkpoints/__init__.py"].replace(
            '''    except (OSError, ValueError):
        raise RegistryError(f"{path}: registry is not valid JSON") from None
''',
            '''    except (OSError, ValueError):
        return empty()
''',
        ).replace(
            '''    if not isinstance(data, dict) or set(data) != {"checkpoints", "latest"} or not isinstance(data["checkpoints"], list):
        raise shape
''',
            '''    if not isinstance(data, dict) or set(data) != {"checkpoints", "latest"} or not isinstance(data["checkpoints"], list):
        return empty()
''',
        ),
        "checkpoints/__main__.py": REFERENCE["checkpoint-registry"]["checkpoints/__main__.py"],
    },
    "ledger-compactor": {
        # Treats a corrupt snapshot as empty and overwrites it.
        "ledger/__init__.py": REFERENCE["ledger-compactor"]["ledger/__init__.py"].replace(
            '''    except (OSError, ValueError):
        raise LedgerError(f"{path}: snapshot is not valid JSON") from None
''',
            '''    except (OSError, ValueError):
        return {"accounts": {}, "through_seq": 0}
''',
        ).replace(
            '''    if not isinstance(data, dict) or set(data) != {"accounts", "through_seq"} or not isinstance(data["accounts"], dict):
        raise shape
''',
            '''    if not isinstance(data, dict) or set(data) != {"accounts", "through_seq"} or not isinstance(data["accounts"], dict):
        return {"accounts": {}, "through_seq": 0}
''',
        ),
        "ledger/__main__.py": REFERENCE["ledger-compactor"]["ledger/__main__.py"],
    },
}


def families() -> dict[str, dict[str, list[dict]]]:
    manifest = benchmark.load_manifest()
    grouped: dict[str, dict[str, list[dict]]] = {}
    for track, task in benchmark.tasks(manifest):
        if task.get("family"):
            grouped.setdefault(task["family"], {"source": [], "held_out": [], "track": track})[task["transfer_role"]].append(task)
    return grouped


def fixture(task: dict) -> Path:
    return ROOT / task["fixture"]


def fixture_files(task: dict) -> dict[str, str]:
    root = fixture(task)
    return {path.relative_to(root).as_posix(): path.read_text(encoding="utf-8") for path in root.rglob("*") if path.is_file()}


def mentions(text: str, literal: str) -> bool:
    return re.search(r"(?<![A-Za-z0-9_])" + re.escape(literal) + r"(?![A-Za-z0-9_])", text) is not None


class ConstructionAuditTests(unittest.TestCase):
    """Structural distinctness of every declared source/held-out pair."""

    def setUp(self):
        self.families = families()
        self.assertEqual(sorted(self.families), ["atomic-state-update", "cli-error-contract"])

    def pairs(self):
        for family, members in self.families.items():
            source = members["source"][0]
            for held_out in members["held_out"]:
                yield family, source, held_out

    def test_catalog_metadata_is_valid_and_pairs_are_distinct(self):
        benchmark.validate_manifest(benchmark.load_manifest())
        for family, source, held_out in self.pairs():
            with self.subTest(family=family):
                self.assertNotEqual(source["id"], held_out["id"])
                self.assertNotEqual(source["protected_sha256"], held_out["protected_sha256"])
                self.assertEqual(source["protected_sha256"], benchmark.protected_digest(fixture(source), source["protected_paths"]))
                self.assertEqual(held_out["protected_sha256"], benchmark.protected_digest(fixture(held_out), held_out["protected_paths"]))
                self.assertNotEqual(source["editable_paths"], held_out["editable_paths"])
                self.assertEqual(sorted(source["protected_paths"]), ["README.md", "tests"])
                self.assertEqual(sorted(held_out["protected_paths"]), ["README.md", "tests"])

    def test_filenames_do_not_mirror_each_other(self):
        for family, source, held_out in self.pairs():
            with self.subTest(family=family):
                shared = set(fixture_files(source)) & set(fixture_files(held_out))
                self.assertEqual(shared, {"README.md"})
                self.assertNotEqual(source["editable_paths"][0], held_out["editable_paths"][0])

    def test_task_specific_vocabulary_does_not_cross_the_pair(self):
        for family, source, held_out in self.pairs():
            for own, other in ((source, held_out), (held_out, source)):
                other_text = "\n".join(fixture_files(other).values())
                for literal in SPECIFICS[own["id"]]:
                    with self.subTest(family=family, task=own["id"], literal=literal):
                        self.assertTrue(any(mentions(text, literal) for text in fixture_files(own).values()), f"{literal} is not even used by {own['id']}")
                        self.assertFalse(mentions(other_text, literal), f"{literal} from {own['id']} appears in {other['id']}")

    def test_starter_packages_carry_no_implementation(self):
        for members in self.families.values():
            for task in members["source"] + members["held_out"]:
                with self.subTest(task=task["id"]):
                    package = fixture(task) / task["editable_paths"][0]
                    sources = list(package.rglob("*.py"))
                    self.assertEqual([path.name for path in sources], ["__init__.py"])
                    text = sources[0].read_text(encoding="utf-8")
                    self.assertNotRegex(text, r"^(def |class |import |from )", "starter package must be a docstring only")
                    self.assertEqual(len(text.splitlines()), 1)

    def test_held_out_graders_never_touch_the_source_task(self):
        for family, source, held_out in self.pairs():
            with self.subTest(family=family):
                tests = "\n".join(text for name, text in fixture_files(held_out).items() if name.startswith("tests/"))
                self.assertFalse(mentions(tests, source["editable_paths"][0]))
                self.assertNotIn(source["id"], tests)
                self.assertNotIn("tests/benchmarks", tests)
                self.assertNotIn("sys.path", tests)
                self.assertNotIn("importlib", tests)
                # The candidate is launched as a module from its own workspace only.
                self.assertIn('"PYTHONPATH": str(ROOT)', tests)

    def test_expected_test_counts_match_the_protected_suites(self):
        for members in self.families.values():
            for task in members["source"] + members["held_out"]:
                with self.subTest(task=task["id"]):
                    count = sum(text.count("def test_") for name, text in fixture_files(task).items() if name.startswith("tests/"))
                    self.assertEqual(count, task["expected_tests"])
                    self.assertEqual(task["grader"], ["python3", "-m", "unittest", "discover", "-s", "tests", "-v"])


class GraderValidationTests(unittest.TestCase):
    """Prepare, digest, untouched, reference, wrong and undeclared-path outcomes for every task."""

    def setUp(self):
        WORK_ROOT.mkdir(parents=True, exist_ok=True)
        self.work = Path(tempfile.mkdtemp(prefix="harness-transfer-test.", dir=WORK_ROOT))
        self.addCleanup(shutil.rmtree, self.work, True)

    def tool(self, *arguments: str) -> subprocess.CompletedProcess:
        return subprocess.run([sys.executable, str(TOOL), *arguments], check=False, text=True, capture_output=True)

    def prepare(self, task: dict, name: str) -> Path:
        workspace = self.work / task["id"] / name
        completed = self.tool("prepare", "--track", "project", "--task", task["id"], "--destination", str(workspace))
        self.assertEqual(completed.returncode, 0, completed.stderr)
        return workspace

    def grade(self, task: dict, workspace: Path) -> dict:
        completed = self.tool("grade", "--track", "project", "--task", task["id"], "--workspace", str(workspace), "--timeout", "120")
        lines = [line for line in completed.stdout.splitlines() if line.strip()]
        self.assertTrue(lines, completed.stderr)
        result = json.loads(lines[-1])
        self.assertEqual(completed.returncode, 0 if result["passed"] else 2)
        return result

    def install(self, workspace: Path, files: dict[str, str]) -> None:
        for relative, text in files.items():
            (workspace / relative).write_text(text, encoding="utf-8")

    def test_every_transfer_task_is_preparable_gradable_and_discriminating(self):
        for members in families().values():
            for task in members["source"] + members["held_out"]:
                with self.subTest(task=task["id"]):
                    untouched = self.grade(task, self.prepare(task, "untouched"))
                    self.assertFalse(untouched["passed"])
                    self.assertEqual(untouched["observed_tests"], task["expected_tests"], untouched)
                    reference = self.prepare(task, "reference")
                    self.install(reference, REFERENCE[task["id"]])
                    result = self.grade(task, reference)
                    self.assertTrue(result["passed"], result)
                    self.assertEqual(result["observed_tests"], task["expected_tests"])
                    wrong = self.prepare(task, "wrong")
                    self.install(wrong, WRONG[task["id"]])
                    result = self.grade(task, wrong)
                    self.assertFalse(result["passed"], result)
                    self.assertEqual(result["observed_tests"], task["expected_tests"], result)
                    undeclared = self.prepare(task, "undeclared")
                    self.install(undeclared, REFERENCE[task["id"]])
                    (undeclared / "notes.txt").write_text("stray\n", encoding="utf-8")
                    result = self.grade(task, undeclared)
                    self.assertFalse(result["passed"])
                    self.assertIn("undeclared path", result["reason"])


if __name__ == "__main__":
    unittest.main()
