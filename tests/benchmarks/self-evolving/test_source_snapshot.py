from pathlib import Path
import json
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import run as runner


class SourceSnapshotTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / 'repo'
        self.root.mkdir()
        self.git('init', '-q')
        self.output = Path(self.temp.name) / 'snapshot'

    def git(self, *args):
        subprocess.run(['git', '-C', str(self.root), *args], check=True, capture_output=True)

    def snapshot(self):
        with patch.object(runner, 'ROOT', self.root):
            return runner.snapshot_source(self.output)

    def test_index_and_working_contents_define_snapshot(self):
        (self.root/'tracked').write_text('committed')
        (self.root/'.gitignore').write_text('ignored\n')
        self.git('add', '.')
        self.git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'Initial')
        (self.root/'staged').write_text('new source')
        self.git('add', 'staged')
        (self.root/'personal-document').write_text('private data')
        (self.root/'ignored').write_text('ignored data')
        before = self.snapshot()
        self.assertEqual(set(json.loads((self.output/'source.json').read_text())), {'.gitignore', 'staged', 'tracked'})
        self.assertFalse((self.output/'source/personal-document').exists())
        self.assertFalse((self.output/'source/ignored').exists())
        (self.root/'tracked').write_text('working edit')
        self.assertNotEqual(before, self.snapshot())
        self.assertEqual((self.output/'source/tracked').read_text(), 'working edit')

    def test_missing_tracked_file_fails_without_partial_snapshot(self):
        (self.root/'a').write_text('source')
        (self.root/'z').write_text('source')
        self.git('add', '.')
        (self.root/'z').unlink()
        with self.assertRaisesRegex(RuntimeError, 'missing'):
            self.snapshot()
        self.assertFalse(self.output.exists())

    def test_tracked_symlink_fails_without_following_private_target(self):
        private = Path(self.temp.name)/'private'
        private.write_text('private data')
        (self.root/'link').symlink_to(private)
        self.git('add', 'link')
        with self.assertRaisesRegex(RuntimeError, 'symlink'):
            self.snapshot()
        self.assertFalse(self.output.exists())

    def test_symlink_parent_cannot_redirect_tracked_path(self):
        (self.root/'directory').mkdir()
        (self.root/'directory/file').write_text('tracked data')
        self.git('add', '.')
        (self.root/'directory/file').unlink()
        (self.root/'directory').rmdir()
        private = Path(self.temp.name)/'private-dir'
        private.mkdir()
        (private/'file').write_text('private data')
        (self.root/'directory').symlink_to(private, target_is_directory=True)
        with self.assertRaisesRegex(RuntimeError, 'symlink'):
            self.snapshot()
        self.assertFalse(self.output.exists())

    def test_oversized_tracked_file_fails_instead_of_silent_omission(self):
        (self.root/'large').write_bytes(b'x' * 5_000_001)
        self.git('add', 'large')
        with self.assertRaisesRegex(RuntimeError, 'snapshot limit'):
            self.snapshot()
        self.assertFalse(self.output.exists())


if __name__ == '__main__':
    unittest.main()
