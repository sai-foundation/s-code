#!/usr/bin/env python3
"""Verify the standalone generated Community repository and export manifest."""

from __future__ import annotations

import hashlib
import json
import re
import stat
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
HEX_REVISION = re.compile(r"^[0-9a-f]{40}$")
PLACEHOLDERS = ("OWNER/REPOSITORY", "example/opencoding", "security@opencoding.example")


def fail(message: str) -> None:
    raise ValueError(message)


def load(path: Path) -> dict[str, object]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        fail(f"{path.name} must contain an object")
    return value


def matches(relative: str, candidate: str) -> bool:
    return relative == candidate or relative.startswith(f"{candidate}/")


def main() -> int:
    try:
        contract = load(ROOT / "community-release.json")
        manifest = load(ROOT / "COMMUNITY-EXPORT.json")
        if manifest.get("schema_version") != 2 or manifest.get("edition") != "community":
            fail("unsupported Community export manifest")
        repository = contract["public_repository"]
        identity = contract["repository_identity"]
        if identity.get("url") != "https://github.com/shilongliu-iteria/opencoding-community":
            fail("canonical repository identity is invalid")
        if identity.get("publication_enabled") is not False:
            fail("public publication must remain disabled in private staging")
        revision = manifest.get("source_revision")
        if not isinstance(revision, str) or HEX_REVISION.fullmatch(revision) is None:
            fail("export manifest has no full source revision")
        if manifest.get("source_dirty") is not False:
            fail("standalone Community repository was generated from a dirty source tree")
        if manifest.get("contract_sha256") != hashlib.sha256(
            (ROOT / "community-release.json").read_bytes()
        ).hexdigest():
            fail("export manifest does not bind the release contract")
        if manifest.get("export_lock_sha256") != hashlib.sha256(
            (ROOT / "Cargo.lock").read_bytes()
        ).hexdigest():
            fail("export manifest does not bind the exported Cargo.lock")

        for relative in repository["required_files"]:
            if not (ROOT / relative).is_file():
                fail(f"required file is missing: {relative}")
        for relative in repository["required_directories"]:
            if not (ROOT / relative).is_dir():
                fail(f"required directory is missing: {relative}")

        forbidden = repository["forbidden_integration_paths"]
        forbidden_licenses = repository["forbidden_license_identifiers"]
        expected_files = manifest.get("files")
        expected_modes = manifest.get("file_modes")
        if not isinstance(expected_files, dict) or not expected_files:
            fail("export manifest file map is missing")
        if not isinstance(expected_modes, dict) or set(expected_modes) != set(expected_files):
            fail("export manifest mode map is missing or incomplete")
        actual_files: dict[str, str] = {}
        actual_modes: dict[str, str] = {}
        if (ROOT / ".git").exists():
            tracked = set(
                subprocess.check_output(
                    ["git", "ls-files"], cwd=ROOT, text=True
                ).splitlines()
            )
            expected_tracked = set(expected_files) | {"COMMUNITY-EXPORT.json"}
            if tracked != expected_tracked:
                fail(
                    "tracked file set differs from the generated manifest "
                    f"missing={sorted(expected_tracked - tracked)} "
                    f"extra={sorted(tracked - expected_tracked)}"
                )
            candidates = [ROOT / relative for relative in sorted(expected_files)]
        else:
            candidates = sorted(ROOT.rglob("*"))
        for path in candidates:
            relative = path.relative_to(ROOT).as_posix()
            if relative == ".git" or relative.startswith(".git/"):
                continue
            if path.is_symlink():
                fail(f"symlink is forbidden: {relative}")
            if any(matches(relative, candidate) for candidate in forbidden):
                fail(f"integration-only path is present: {relative}")
            if not path.is_file() or relative == "COMMUNITY-EXPORT.json":
                continue
            if path.stat().st_size > repository["maximum_file_bytes"]:
                fail(f"file exceeds size policy: {relative}")
            contents = path.read_bytes()
            actual_files[relative] = hashlib.sha256(contents).hexdigest()
            actual_modes[relative] = format(stat.S_IMODE(path.stat().st_mode), "04o")
            try:
                text = contents.decode("utf-8")
            except UnicodeError:
                continue
            if relative != "community-release.json" and any(
                identifier in text for identifier in forbidden_licenses
            ):
                fail(f"Enterprise license identifier is present: {relative}")
            if path.suffix in {".md", ".sh", ".yml", ".yaml"} and any(
                placeholder in text for placeholder in PLACEHOLDERS
            ):
                fail(f"release placeholder is present: {relative}")
        if actual_files != expected_files:
            missing = sorted(set(expected_files) - set(actual_files))
            extra = sorted(set(actual_files) - set(expected_files))
            changed = sorted(
                name
                for name in set(expected_files) & set(actual_files)
                if expected_files[name] != actual_files[name]
            )
            fail(f"export manifest drift missing={missing} extra={extra} changed={changed}")
        if actual_modes != expected_modes:
            changed_modes = sorted(
                name for name in actual_modes if actual_modes[name] != expected_modes[name]
            )
            fail(f"export file mode drift: {changed_modes}")
    except (
        OSError,
        KeyError,
        TypeError,
        ValueError,
        json.JSONDecodeError,
        subprocess.CalledProcessError,
    ) as error:
        print(f"Community tree verification failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps({"passed": True, "files": len(actual_files), "source_revision": revision}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
