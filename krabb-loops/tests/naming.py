"""Infer contribution names without changing existing branches or invoking a provider."""

import argparse
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('contribute', ROOT / 'krabb-loops/lib/contribute.py')
MODULE = importlib.util.module_from_spec(SPEC)

SPEC.loader.exec_module(MODULE)


class Naming(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()

        self.addCleanup(self.directory.cleanup)

        self.root = Path(self.directory.name) / 'work'
        remote = Path(self.directory.name) / 'remote.git'

        self.root.mkdir()
        self.git('init', '--initial-branch=main')
        self.git('config', 'user.name', 'Fixture')
        self.git('config', 'user.email', 'fixture@example.invalid')
        self.git('config', 'commit.gpgsign', 'false')
        self.git('config', 'core.hooksPath', '/dev/null')
        (self.root / 'file').write_text('initial')
        self.git('add', '.')
        self.git('commit', '-m', 'chore: initial')
        self.git('init', '--bare', str(remote))
        self.git('remote', 'add', 'origin', str(remote))
        self.git('push', '-u', 'origin', 'main')

        self.loop = object.__new__(MODULE.Contribution)
        self.loop.repo = 'owner/repo'
        self.loop.base = 'main'
        self.loop.branch = ''
        self.loop.sha = self.git('rev-parse', 'HEAD')
        self.loop.attempt = 0
        self.loop.state = self.root / '.git/evidence'
        self.loop.args = argparse.Namespace(branch=None, max_repairs=0)
        self.loop.agent = Mock(return_value='multiprovider')

    def git(self, *args):
        return subprocess.run(['git', *args], cwd=self.root, capture_output=True,
                              text=True, check=True).stdout.strip()

    def prepare(self, existing=None):
        with patch.object(MODULE, 'ROOT', self.root):
            with patch.object(MODULE, 'gh', return_value=existing or []):
                self.loop.prepare()

    def test_inferred_name_preserves_staged_unstaged_and_untracked_changes(self):
        (self.root / 'file').write_text('staged')
        self.git('add', 'file')
        (self.root / 'file').write_text('unstaged')
        (self.root / 'new').write_text('untracked')

        before = self.git('status', '--porcelain')
        index = self.git('write-tree')

        def infer(prompt):
            self.loop.stable()

            self.assertIn('staged, unstaged and untracked', prompt)
            self.assertEqual(self.git('branch', '--show-current'), 'main')

            return 'multiprovider\n'

        self.loop.agent.side_effect = infer

        self.prepare()

        self.assertEqual(self.git('branch', '--show-current'), 'multiprovider')
        self.assertEqual(self.loop.branch, 'multiprovider')
        self.assertEqual(self.git('status', '--porcelain'), before)
        self.assertEqual(self.git('write-tree'), index)
        self.assertEqual(self.git('rev-parse', 'HEAD'), self.loop.sha)

    def test_invalid_and_occupied_names_are_regenerated(self):
        self.git('branch', 'routing')
        self.git('push', 'origin', 'HEAD:refs/heads/providers')
        self.git('branch', 'approvals/previous')

        self.loop.agent.side_effect = [
            'multi-provider', 'Multiprovider', 'x' * 33, 'name\nexplanation', '',
            'main', 'routing', 'providers', 'approvals', 'multiprovider',
        ]

        self.prepare()

        self.assertEqual(self.loop.branch, 'multiprovider')
        self.assertEqual(self.loop.attempt, 9)
        self.assertIn('already reserved', self.loop.agent.call_args.args[0])
        self.assertEqual(self.git('rev-parse', 'routing'), self.loop.sha)

    def test_explicit_override_does_not_infer(self):
        self.loop.args.branch = 'my-topic'

        self.prepare()

        self.assertEqual(self.loop.branch, 'my-topic')
        self.loop.agent.assert_not_called()

    def test_existing_pr_is_resumed_without_naming(self):
        self.git('switch', '-c', 'contrib-previous')

        self.prepare(existing=[{'number': 42}])

        self.assertEqual(self.loop.branch, 'contrib-previous')
        self.loop.agent.assert_not_called()

    def test_explicit_current_branch_resumes_without_naming(self):
        self.git('switch', '-c', 'multiprovider')

        self.loop.args.branch = 'multiprovider'

        self.prepare()

        self.assertEqual(self.loop.branch, 'multiprovider')
        self.loop.agent.assert_not_called()

    def test_invalid_names_obey_recovery_budget(self):
        self.loop.args.max_repairs = 1
        self.loop.agent.return_value = 'invalid name'

        with self.assertRaisesRegex(RuntimeError, 'repair limit'):
            self.prepare()

        self.assertEqual(self.git('branch', '--show-current'), 'main')
        self.assertEqual(self.loop.agent.call_count, 2)

    def test_content_change_during_naming_retries_inference(self):
        self.loop.agent.side_effect = [MODULE.Reverify('content changed'), 'multiprovider']

        self.prepare()

        self.assertEqual(self.loop.branch, 'multiprovider')
        self.assertEqual(self.loop.attempt, 1)


if __name__ == '__main__':
    unittest.main()
