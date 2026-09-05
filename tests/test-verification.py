#!/usr/bin/env python3
"""Exercise source-archive and populated-checkout verification boundaries."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class VerificationTests(unittest.TestCase):
    def setUp(self):
        (ROOT / ".work").mkdir(exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(prefix="verification-test-", dir=ROOT / ".work")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name) / "source"
        self.root.mkdir()
        (self.root / "scripts").mkdir()
        for name in ("verify-community.sh", "check-community-tree.py", "check-community-secrets.py", "check-doc-links.py"):
            shutil.copy2(ROOT / "scripts" / name, self.root / "scripts" / name)
        contract = json.loads((ROOT / "community-release.json").read_text())
        # Keep the real boundary policy, with a small source fixture.
        contract["public_repository"]["required_files"] = ["README.md"]
        contract["public_repository"]["required_directories"] = ["scripts", "docs", "clients", "compliance"]
        (self.root / "community-release.json").write_text(json.dumps(contract))
        for path in ("docs", "clients/vscode", "compliance"):
            (self.root / path).mkdir(parents=True)
        (self.root / "README.md").write_text("[Guide](docs/guide.md)\n")
        (self.root / "docs/guide.md").write_text("Source fixture\n")
        (self.root / ".gitignore").write_text(".work/\nnode_modules/\n.vscode-test/\n")

    def run_script(self, script, *args, environment=None):
        return subprocess.run(["sh" if script.endswith(".sh") else "python3", str(self.root / "scripts" / script), *args],
                              cwd=self.root, env=environment, text=True, capture_output=True)

    def git(self, *args):
        subprocess.run(["git", "-C", str(self.root), *args], check=True, capture_output=True)

    def test_fresh_archive_passes_preflight_before_creating_scratch(self):
        for scope in ("all", "policy"):
            fakebin = Path(self.temporary.name) / "bin"
            fakebin.mkdir(exist_ok=True)
            cargo = fakebin / "cargo"
            cargo.write_text("#!/bin/sh\necho reached-cargo >&2\nexit 73\n")
            cargo.chmod(0o755)
            environment = os.environ.copy()
            environment["PATH"] = str(fakebin) + os.pathsep + environment["PATH"]
            result = self.run_script("verify-community.sh", scope, environment=environment)
            self.assertNotEqual(result.returncode, 0)  # Stop at the fake build tool.
            self.assertIn("validated local links", result.stdout, result.stderr)
            self.assertNotIn("Community tree verification failed", result.stderr)
            shutil.rmtree(self.root / ".work")

    def test_archive_still_rejects_unexpected_integration_content(self):
        (self.root / "crates/control-plane").mkdir(parents=True)
        result = self.run_script("verify-community.sh", "policy")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("integration-only path", result.stderr)
        self.assertFalse((self.root / ".work").exists())

    def test_git_checkout_ignores_installed_docs_but_checks_new_source_docs(self):
        self.git("init", "-q")
        self.git("add", ".")
        self.git("-c", "user.name=Verification Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "source")
        self.install_vendor_docs()
        result = self.run_script("check-doc-links.py", "README.md", "docs", "clients", "compliance")
        self.assertEqual(result.returncode, 0, result.stderr)
        (self.root / "clients/new-guide.md").write_text("[Broken source](missing.md)\n")
        result = self.run_script("check-doc-links.py", "clients")
        self.assertEqual(result.returncode, 1)
        self.assertIn("clients/new-guide.md", result.stderr)
        self.assertNotIn("vendor/README.md", result.stderr)

    def test_archive_docs_skip_generated_dependencies_and_support_default_selection(self):
        self.install_vendor_docs()
        for args in [(), ("README.md", "docs", "clients", "compliance")]:
            result = self.run_script("check-doc-links.py", *args)
            self.assertEqual(result.returncode, 0, result.stderr)

    def install_vendor_docs(self):
        for path in ("clients/vscode/node_modules/vendor", "clients/vscode/.vscode-test/vendor"):
            directory = self.root / path
            directory.mkdir(parents=True)
            (directory / "README.md").write_text("[Not shipped](missing-vendor-file.md)\n")


if __name__ == "__main__":
    unittest.main()
