#!/usr/bin/env python3
"""Hermetic dependency bootstrap: no real packages, downloads or user state."""
import hashlib
import os
import pty
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
NODE = 'node-v22.23.2-linux-x64'
NODE_HASH = 'b294a556e639d64338823920e5866c21c02741742d2e1529ee1a225c1ec9252a'
RUSTUP_HASH = '17247e4bcacf6027ec2e11c79a72c494c9af69ac8d1abcc1b271fa4375a106c2'


def executable(path, content):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text('#!/bin/sh\nset -eu\n' + content)
    path.chmod(0o755)


class BootstrapTests(unittest.TestCase):
    def setUp(self):
        (ROOT / '.work').mkdir(exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=ROOT / '.work', prefix='bootstrap-test.')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.bin = self.root / 'bin'
        self.home = self.root / 'home'
        self.ready = self.root / 'ready'
        self.scripts = self.root / 'scripts'
        for path in (self.bin, self.home, self.ready, self.scripts):
            path.mkdir()
        self.actions = self.root / 'actions'
        self.helper = self.scripts / 'source-dependencies.sh'
        self.helper.write_text((ROOT / 'scripts/source-dependencies.sh').read_text())
        shutil.copy(ROOT / 'rust-toolchain.toml', self.root)
        self.driver = self.scripts / 'install-from-source.sh'
        executable(self.driver, 'ROOT=' + shlex.quote(str(self.root)) + '''
SOURCE_DEPENDENCY_MODE=prompt
case "${1:-}" in --yes) SOURCE_DEPENDENCY_MODE=yes ;; --check-deps) SOURCE_DEPENDENCY_MODE=check ;; --no-install-deps) SOURCE_DEPENDENCY_MODE=never ;; esac
. "$ROOT/scripts/source-dependencies.sh"
prepare_source_dependencies
printf 'bootstrap completed\\n'
''')
        self.env = {'PATH': str(self.bin), 'HOME': str(self.home), 'FIXTURE': str(self.root),
                    'LC_ALL': 'C', 'FIXTURE_OS': 'Linux', 'FIXTURE_ARCH': 'x86_64'}
        for name in ('grep', 'sed', 'mkdir', 'mktemp', 'rm', 'mv', 'tar', 'gzip', 'install', 'sh'):
            (self.bin / name).symlink_to(shutil.which(name))
        executable(self.bin / 'id', 'echo 1000\n')
        executable(self.bin / 'uname', '[ "${1:-}" != -m ] || { echo "$FIXTURE_ARCH"; exit 0; }; echo "$FIXTURE_OS"\n')
        for name in ('git', 'cc', 'c++', 'make', 'bwrap'):
            executable(self.bin / name, 'exit 0\n')
        executable(self.bin / 'python3', 'exec ' + shlex.quote(sys.executable) + ' "$@"\n')
        for name, marker, version in [('rustc', 'rust', 'rustc 1.89.0 (fixture)'),
                                      ('cargo', 'rust', 'cargo 1.89.0'),
                                      ('node', 'node', 'v22.23.2'), ('npm', 'node', '10.9.8')]:
            executable(self.bin / name, '[ -f "$FIXTURE/ready/' + marker + '" ] || exit 1\nprintf "%s\\n" ' + shlex.quote(version))
        executable(self.bin / 'curl', '''
printf 'curl %s\\n' "$*" >> "$FIXTURE/actions"
output=; url=
while [ "$#" -gt 0 ]; do
  case "$1" in --output) output=$2; shift ;; https:*) url=$1 ;; esac
  shift
done
case "$url" in
  *nodejs.org*) exec /bin/cp "$FIXTURE/node.tar.gz" "$output" ;;
  *rustup-init.sh) exec /bin/cp "$FIXTURE/rustup-init.sh" "$output" ;;
  *) exit 99 ;;
esac
''')

    def tools_ready(self, *names):
        for name in names:
            (self.ready / name).touch()

    def run_bootstrap(self, *args, success=True):
        result = subprocess.run([str(self.driver), *args], env=self.env, text=True,
                                capture_output=True, input='', timeout=20)
        if success:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(result.stdout.count('bootstrap completed'), 1)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def fake_node_archive(self, valid=True):
        tree = self.root / NODE
        executable(tree / 'bin/node', 'echo v22.23.2\n')
        executable(tree / 'bin/npm', 'echo 10.9.8\n')
        archive = self.root / 'node.tar.gz'
        with tarfile.open(archive, 'w:gz') as output:
            output.add(tree, arcname=NODE)
        if valid:
            self.helper.write_text(self.helper.read_text().replace(NODE_HASH, hashlib.sha256(archive.read_bytes()).hexdigest()))

    def test_ready_modes_have_no_install_side_effects(self):
        self.tools_ready('rust', 'node')
        self.run_bootstrap()
        self.run_bootstrap('--check-deps')
        self.assertFalse(self.actions.exists())
        self.assertFalse((self.home / '.cache/s-code').exists())
        self.assertFalse((self.home / '.cargo').exists())

    def test_missing_tools_require_consent(self):
        for mode in ((), ('--check-deps',), ('--no-install-deps',)):
            result = self.run_bootstrap(*mode, success=False)
            self.assertIn('Rust 1.89.0', result.stdout)
            self.assertIn('Node.js 22.23.2', result.stdout)
        self.assertFalse(self.actions.exists())
        self.assertFalse((self.home / '.cache/s-code').exists())
        self.assertFalse((self.home / '.cargo').exists())

    def test_rust_probe_disables_implicit_downloads(self):
        executable(self.bin / 'rustc', '''
[ "$RUSTUP_AUTO_INSTALL" = 0 ] || { echo implicit-download >> "$FIXTURE/actions"; exit 99; }
[ "$RUSTUP_TOOLCHAIN" = 1.89.0 ] || exit 98
exit 1
''')
        self.run_bootstrap('--check-deps', success=False)
        self.assertFalse(self.actions.exists())

    def test_interactive_decline_does_not_install(self):
        master, slave = pty.openpty()
        try:
            process = subprocess.Popen([str(self.driver)], env=self.env, stdin=slave,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            os.write(master, b'n\n')
            stdout, stderr = process.communicate(timeout=20)
            self.assertNotEqual(process.returncode, 0, stdout + stderr)
            self.assertIn('Installation cancelled', stdout)
            self.assertFalse(self.actions.exists())
        finally:
            os.close(master)
            os.close(slave)

    def test_node_is_private_and_reused(self):
        self.tools_ready('rust')
        self.fake_node_archive()
        before = (self.bin / 'node').read_bytes()
        self.run_bootstrap('--yes')
        self.assertTrue((self.home / '.cache/s-code/build-tools' / NODE / 'bin/node').is_file())
        self.assertEqual((self.bin / 'node').read_bytes(), before)
        downloads = self.actions.read_text()
        self.run_bootstrap('--check-deps')
        self.assertEqual(self.actions.read_text(), downloads)
        self.assertFalse((self.home / '.profile').exists())

    def test_bad_checksum_never_publishes_node(self):
        self.tools_ready('rust')
        self.fake_node_archive(valid=False)
        result = self.run_bootstrap('--yes', success=False)
        self.assertIn('checksum mismatch', result.stderr)
        tools = self.home / '.cache/s-code/build-tools'
        self.assertFalse((tools / NODE).exists())
        self.assertEqual(list(tools.glob('.node.*')), [])

    def test_concurrent_bootstraps_share_one_complete_node(self):
        self.tools_ready('rust')
        self.fake_node_archive()
        processes = [subprocess.Popen([str(self.driver), '--yes'], env=self.env,
                     stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) for _ in range(2)]
        try:
            for process in processes:
                stdout, stderr = process.communicate(timeout=20)
                self.assertEqual(process.returncode, 0, stdout + stderr)
            self.assertEqual(self.actions.read_text().count('curl '), 1)
        finally:
            for process in processes:
                if process.poll() is None:
                    process.kill()
                    process.wait()

    def test_existing_rustup_keeps_default(self):
        self.tools_ready('node')
        executable(self.bin / 'rustup', '''
printf 'rustup %s\\n' "$*" >> "$FIXTURE/actions"
[ "$*" = 'toolchain install 1.89.0 --profile minimal --no-self-update' ] || exit 99
: > "$FIXTURE/ready/rust"
''')
        self.run_bootstrap('--yes')
        self.assertNotIn('default', self.actions.read_text())
        self.assertNotIn('curl', self.actions.read_text())

    def test_new_rustup_is_isolated(self):
        self.tools_ready('node')
        init = self.root / 'rustup-init.sh'
        executable(init, '''
printf 'rustup-init %s\\n' "$*" >> "$FIXTURE/actions"
[ "$RUSTUP_VERSION" = 1.28.2 ] || exit 98
[ "$CARGO_HOME" = "$HOME/.cache/s-code/build-tools/cargo" ] || exit 99
mkdir -p "$CARGO_HOME/bin" "$RUSTUP_HOME"
for tool in rustup rustc cargo; do
  /bin/cp "$FIXTURE/bin/rustc" "$CARGO_HOME/bin/$tool"
done
: > "$FIXTURE/ready/rust"
''')
        self.helper.write_text(self.helper.read_text().replace(RUSTUP_HASH, hashlib.sha256(init.read_bytes()).hexdigest()))
        self.run_bootstrap('--yes')
        self.assertIn('--no-modify-path', self.actions.read_text())
        self.assertFalse((self.home / '.cargo').exists())
        self.assertFalse((self.home / '.profile').exists())

    def test_linux_system_packages(self):
        self.tools_ready('rust', 'node')
        (self.bin / 'bwrap').unlink()
        executable(self.bin / 'sudo', 'printf "sudo %s\\n" "$*" >> "$FIXTURE/actions"\nexec "$@"\n')
        executable(self.bin / 'apt-get', '''
printf 'apt-get %s\\n' "$*" >> "$FIXTURE/actions"
[ "$1" != install ] || /bin/cp "$FIXTURE/bin/git" "$FIXTURE/bin/bwrap"
''')
        self.run_bootstrap('--yes')
        self.assertIn('sudo apt-get update', self.actions.read_text())
        self.assertIn('bubblewrap', self.actions.read_text())

    def test_unknown_package_manager_stops(self):
        (self.bin / 'bwrap').unlink()
        self.run_bootstrap('--yes', success=False)
        self.assertFalse(self.actions.exists())

    def test_macos_developer_tools_need_system_completion(self):
        self.tools_ready('rust', 'node')
        self.env['FIXTURE_OS'] = 'Darwin'
        executable(self.bin / 'xcode-select', '''
[ "$1" != -p ] || exit 1
printf 'xcode-select %s\\n' "$*" >> "$FIXTURE/actions"
''')
        result = self.run_bootstrap('--yes', success=False)
        self.assertIn("Complete Apple's", result.stderr)
        self.assertEqual(self.actions.read_text().strip(), 'xcode-select --install')

    def test_unsupported_os_and_architecture_stop(self):
        self.env['FIXTURE_OS'] = 'Windows_NT'
        self.run_bootstrap('--yes', success=False)
        self.env['FIXTURE_OS'] = 'Linux'
        self.env['FIXTURE_ARCH'] = 'riscv64'
        self.run_bootstrap('--yes', success=False)
        self.assertFalse(self.actions.exists())

    def test_unsafe_managed_directory_is_rejected(self):
        self.tools_ready('rust')
        parent = self.home / '.cache/s-code'
        parent.mkdir(parents=True)
        (parent / 'build-tools').symlink_to(self.ready)
        result = self.run_bootstrap('--yes', success=False)
        self.assertIn('Build-tools directory', result.stderr)
        self.assertFalse(self.actions.exists())


if __name__ == '__main__':
    unittest.main()
