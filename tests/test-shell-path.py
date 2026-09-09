#!/usr/bin/env python3
"""Exercise installed-command discovery in isolated shell homes."""
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
HELPER = ROOT / "scripts/configure-shell-path.py"


class ShellPathTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.home = Path(temporary.name)
        self.bin = self.home / "bin with ' quotes $() `ticks`"
        self.bin.mkdir()
        self.command = self.bin / "s-code"
        self.command.write_text('#!/bin/sh\nprintf "%s\\n" "$PWD" "$@"\n')
        self.command.chmod(0o755)
        self.env = {"HOME": str(self.home), "PATH": "/usr/bin:/bin", "SHELL": "/bin/bash"}

    def configure(self, *args):
        return subprocess.run([sys.executable, str(HELPER), "--install-dir", str(self.bin), *args],
                              env=self.env, text=True, capture_output=True, check=True)

    def shell(self, command, executable="/bin/bash", *args):
        return subprocess.check_output([executable, *args, "-c", command], env=self.env,
                                       cwd=self.home, text=True).strip()

    def test_bash_preserves_existing_profiles_and_is_idempotent(self):
        profile = self.home / ".profile"
        original = "# existing settings without final newline"
        profile.write_text(original)
        self.configure()
        first = profile.read_bytes()
        self.configure()
        self.assertEqual(profile.read_bytes(), first)
        self.assertTrue(first.decode().startswith(original + "\n"))
        self.assertFalse((self.home / ".bash_profile").exists())
        for name in (".bashrc", ".profile"):
            path = shlex.quote(str(self.home / name))
            result = self.shell(f'. {path}; . {path}; command -v s-code; printf "%s" "$PATH"')
            self.assertEqual(result.splitlines()[0], str(self.command))
            self.assertEqual(result.splitlines()[1].split(":").count(str(self.bin)), 1)

    @unittest.skipUnless(shutil.which("zsh"), "zsh is not installed")
    def test_new_zsh_reads_zdotdir_and_finds_command(self):
        zdotdir = self.home / "zsh config"
        self.env.update(SHELL=shutil.which("zsh"), ZDOTDIR=str(zdotdir))
        self.configure()
        self.assertTrue((zdotdir / ".zshrc").is_file())
        self.assertEqual(self.shell("command -v s-code", self.env["SHELL"], "-i"), str(self.command))

    @unittest.skipUnless(shutil.which("fish"), "fish is not installed")
    def test_new_fish_reads_xdg_config(self):
        self.env.update(SHELL=shutil.which("fish"), XDG_CONFIG_HOME=str(self.home / "config"))
        self.configure()
        self.assertEqual(self.shell("command -v s-code", self.env["SHELL"]), str(self.command))

    def test_opt_out_keeps_profiles_untouched_and_prints_working_activation(self):
        result = self.configure("--no-modify-path")
        self.assertFalse((self.home / ".bashrc").exists())
        activate = next(line.strip() for line in result.stdout.splitlines() if "export PATH=" in line)
        self.assertEqual(self.shell(activate + "; command -v s-code"), str(self.command))

    def test_unwritable_profile_does_not_hide_successful_installation(self):
        (self.home / ".bashrc").mkdir()  # Deterministic even when tests run as root.
        result = self.configure()
        self.assertIn("Could not configure", result.stdout)
        self.assertIn("Start now", result.stdout)
        self.assertNotIn("New terminals can run", result.stdout)

    def test_unknown_shell_keeps_profiles_untouched(self):
        self.env["SHELL"] = "/bin/unknown-shell"
        result = self.configure()
        self.assertIn("Shell not recognized", result.stdout)
        self.assertFalse((self.home / ".bashrc").exists())

    def test_existing_symlink_and_permissions_are_preserved(self):
        actual = self.home / "dotfiles-bashrc"
        actual.write_text("# dotfiles\n")
        actual.chmod(0o600)
        (self.home / ".bashrc").symlink_to(actual)
        self.configure()
        self.assertTrue((self.home / ".bashrc").is_symlink())
        self.assertEqual(actual.stat().st_mode & 0o777, 0o600)
        self.assertTrue(actual.read_text().startswith("# dotfiles\n"))

    def test_repository_launcher_preserves_arguments_directory_and_exit_status(self):
        env = dict(self.env, S_CODE_INSTALL_DIR=str(self.bin))
        result = subprocess.check_output([str(ROOT / "s-code"), "prompt with spaces", "--help"],
                                         env=env, cwd=self.home, text=True)
        self.assertEqual(Path(result.splitlines()[0]).resolve(), self.home.resolve())
        self.assertEqual(result.splitlines()[1:], ["prompt with spaces", "--help"])
        self.command.write_text("#!/bin/sh\nexit 42\n")
        self.assertEqual(subprocess.run([str(ROOT / "s-code")], env=env).returncode, 42)

    def test_repository_launcher_reports_missing_install_and_prevents_recursion(self):
        for directory in (self.home / "missing", ROOT):
            result = subprocess.run([str(ROOT / "s-code")],
                                    env=dict(self.env, S_CODE_INSTALL_DIR=str(directory)),
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 1)
            self.assertIn("scripts/install-from-source.sh", result.stderr)


if __name__ == "__main__":
    unittest.main()
