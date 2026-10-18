#!/usr/bin/env python3
"""Focused contract and adversarial tests for the Harbor runtime bundle."""

import gzip
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
RUNTIME_PATH = ROOT / "integrations/harbor/runtime.py"
BUILDER_PATH = ROOT / "integrations/harbor/build-bundle.sh"
SPEC = importlib.util.spec_from_file_location("harbor_bundle_runtime", RUNTIME_PATH)
runtime = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = runtime
SPEC.loader.exec_module(runtime)


class BundleTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="s-code-harbor-bundle-test.")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.counter = 0
        self.payloads = {
            name: f"fixture:{name}\n".encode() for name in runtime.REQUIRED_RUNTIME_FILES
        }

    def manifest(self):
        return {
            "build": {
                "cargo": "cargo 1.89.0",
                "glibc": "glibc 2.36",
                "node": "v22.23.2",
                "npm": "10.9.2",
                "python": "3.12.0",
                "rustc": "rustc 1.89.0",
                "zlib": "1.2.13",
            },
            "files": {
                name: {
                    "mode": "0755",
                    "sha256": hashlib.sha256(contents).hexdigest(),
                    "size": len(contents),
                }
                for name, contents in self.payloads.items()
            },
            "format": runtime.BUNDLE_FORMAT,
            "schema_version": runtime.BUNDLE_SCHEMA_VERSION,
            "source": {
                "commit": "a" * 40,
                "dirty": False,
                "locks": {
                    "Cargo.lock": "c" * 64,
                    "web/package-lock.json": "d" * 64,
                },
                "tree": "b" * 40,
            },
            "target": runtime.SUPPORTED_TARGET,
        }

    def manifest_bytes(self, value=None):
        if value is None:
            value = self.manifest()
        return (
            json.dumps(value, ensure_ascii=True, separators=(",", ":"), sort_keys=True)
            + "\n"
        ).encode()

    def entries(self, manifest_bytes=None):
        if manifest_bytes is None:
            manifest_bytes = self.manifest_bytes()
        return [
            (runtime.MANIFEST_NAME, manifest_bytes, 0o644, tarfile.REGTYPE, ""),
            *[
                (name, contents, 0o755, tarfile.REGTYPE, "")
                for name, contents in self.payloads.items()
            ],
        ]

    def archive(self, entries=None, *, gzip_mtime=0):
        if entries is None:
            entries = self.entries()
        self.counter += 1
        path = self.directory / f"bundle-{self.counter}.tar.gz"
        with path.open("xb") as raw_output:
            with gzip.GzipFile(
                filename="",
                mode="wb",
                compresslevel=9,
                fileobj=raw_output,
                mtime=gzip_mtime,
            ) as compressed:
                with tarfile.open(
                    fileobj=compressed, mode="w|", format=tarfile.USTAR_FORMAT
                ) as output:
                    for name, contents, mode, kind, linkname in entries:
                        member = tarfile.TarInfo(name)
                        member.mode = mode
                        member.mtime = 0
                        member.uid = 0
                        member.gid = 0
                        member.uname = ""
                        member.gname = ""
                        member.type = kind
                        member.linkname = linkname
                        member.size = len(contents) if kind == tarfile.REGTYPE else 0
                        source = io.BytesIO(contents) if kind == tarfile.REGTYPE else None
                        output.addfile(member, source)
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        return path, digest

    def assert_rejected(self, path, digest):
        with self.assertRaises(runtime.BundleValidationError):
            runtime.validate_bundle(path, digest)

    def test_valid_bundle_returns_authenticated_manifest(self):
        path, digest = self.archive()
        manifest = runtime.validate_bundle(path, digest)
        self.assertEqual(manifest.archive_sha256, digest)
        self.assertEqual(manifest.bundle_format, runtime.BUNDLE_FORMAT)
        self.assertEqual(manifest.source_commit, "a" * 40)
        self.assertEqual(manifest.source_tree, "b" * 40)
        self.assertFalse(manifest.source_dirty)
        self.assertEqual(manifest.target, runtime.SUPPORTED_TARGET)
        self.assertEqual(manifest.glibc_version, "glibc 2.36")
        self.assertEqual(tuple(manifest.files), runtime.REQUIRED_RUNTIME_FILES)
        self.assertEqual(manifest.files["s-code"].mode, 0o755)

    def test_outer_digest_and_payload_tampering_are_rejected(self):
        path, digest = self.archive()
        self.assert_rejected(path, "0" * 64)
        for malformed in ("short", "A" * 64):
            self.assert_rejected(path, malformed)

        entries = self.entries()
        name, _, mode, kind, linkname = entries[1]
        entries[1] = (name, b"tampered\n", mode, kind, linkname)
        tampered_path, tampered_digest = self.archive(entries)
        self.assert_rejected(tampered_path, tampered_digest)

    def test_traversal_unexpected_and_duplicate_members_are_rejected(self):
        for unsafe_name in ("../escape", "/absolute", "unexpected"):
            with self.subTest(name=unsafe_name):
                entries = self.entries()
                entries.append((unsafe_name, b"x", 0o644, tarfile.REGTYPE, ""))
                self.assert_rejected(*self.archive(entries))

        entries = self.entries()
        entries.append(entries[1])
        self.assert_rejected(*self.archive(entries))

    def test_links_nonregular_members_and_unsafe_modes_are_rejected(self):
        for kind, linkname in ((tarfile.SYMTYPE, "s-code-cli"), (tarfile.DIRTYPE, "")):
            with self.subTest(kind=kind):
                entries = self.entries()
                entries[1] = ("s-code", b"", 0o755, kind, linkname)
                self.assert_rejected(*self.archive(entries))

        entries = self.entries()
        name, contents, _, kind, linkname = entries[1]
        entries[1] = (name, contents, 0o4755, kind, linkname)
        self.assert_rejected(*self.archive(entries))

    def test_malformed_duplicate_key_and_unsafe_manifests_are_rejected(self):
        valid = self.manifest_bytes()
        duplicate_key = valid.replace(
            b'{"build":', b'{"schema_version":1,"build":', 1
        )
        variants = [b"{not-json", duplicate_key]

        unsafe = self.manifest()
        unsafe["source"]["dirty"] = True
        variants.append(self.manifest_bytes(unsafe))
        unsafe = self.manifest()
        unsafe["target"] = "aarch64-unknown-linux-gnu"
        variants.append(self.manifest_bytes(unsafe))
        unsafe = self.manifest()
        unsafe["files"]["s-code"]["mode"] = "0777"
        variants.append(self.manifest_bytes(unsafe))

        for manifest_bytes in variants:
            with self.subTest(manifest=manifest_bytes[:30]):
                self.assert_rejected(*self.archive(self.entries(manifest_bytes)))

    def test_size_bounds_reject_cheap_oversized_fixtures(self):
        path, digest = self.archive()
        for field in ("_MAX_ARCHIVE_BYTES", "_MAX_UNPACKED_BYTES"):
            with self.subTest(field=field), mock.patch.object(runtime, field, 1):
                self.assert_rejected(path, digest)

        oversized_manifest = b" " * (runtime._MAX_MANIFEST_BYTES + 1)
        self.assert_rejected(*self.archive(self.entries(oversized_manifest)))

        # A tar header can claim a huge member without allocating its contents.
        member = tarfile.TarInfo("s-code")
        member.mode = 0o755
        member.mtime = member.uid = member.gid = 0
        member.size = runtime._MAX_MEMBER_BYTES + 1
        raw_tar = member.tobuf(format=tarfile.USTAR_FORMAT) + b"\0" * tarfile.RECORDSIZE
        self.counter += 1
        huge_header = self.directory / f"bundle-{self.counter}.tar.gz"
        with huge_header.open("xb") as raw_output:
            with gzip.GzipFile(
                filename="", mode="wb", fileobj=raw_output, mtime=0, compresslevel=9
            ) as compressed:
                compressed.write(raw_tar)
        self.assert_rejected(huge_header, hashlib.sha256(huge_header.read_bytes()).hexdigest())

    def test_non_normalized_archive_and_outer_file_types_are_rejected(self):
        self.assert_rejected(*self.archive(gzip_mtime=1))
        path, digest = self.archive()
        link = self.directory / "bundle-link.tar.gz"
        link.symlink_to(path)
        self.assert_rejected(link, digest)
        self.assert_rejected(self.directory, digest)

    def test_validator_cli_and_help(self):
        path, digest = self.archive()
        valid = subprocess.run(
            [sys.executable, str(RUNTIME_PATH), str(path), digest],
            text=True,
            capture_output=True,
            timeout=10,
        )
        self.assertEqual(valid.returncode, 0, valid.stdout + valid.stderr)
        self.assertIn("validated", valid.stdout)

        help_result = subprocess.run(
            [sys.executable, str(RUNTIME_PATH), "--help"],
            text=True,
            capture_output=True,
            timeout=10,
        )
        self.assertEqual(help_result.returncode, 0, help_result.stderr)
        self.assertIn("Validate a pinned S-Code Harbor bundle", help_result.stdout)

        invalid = subprocess.run(
            [sys.executable, str(RUNTIME_PATH), str(path), "0" * 64],
            text=True,
            capture_output=True,
            timeout=10,
        )
        self.assertNotEqual(invalid.returncode, 0)
        self.assertIn("SHA-256 mismatch", invalid.stderr)

    def test_builder_help_and_argument_rejection(self):
        help_result = subprocess.run(
            [str(BUILDER_PATH), "--help"],
            text=True,
            capture_output=True,
            timeout=10,
        )
        self.assertEqual(help_result.returncode, 0, help_result.stderr)
        self.assertIn("Usage: integrations/harbor/build-bundle.sh", help_result.stdout)
        self.assertIn("x86_64-unknown-linux-gnu.tar.gz.sha256", help_result.stdout)

        for arguments in (("--unknown",), ("--help", "extra")):
            with self.subTest(arguments=arguments):
                rejected = subprocess.run(
                    [str(BUILDER_PATH), *arguments],
                    text=True,
                    capture_output=True,
                    timeout=10,
                )
                self.assertEqual(rejected.returncode, 2)


if __name__ == "__main__":
    unittest.main()
