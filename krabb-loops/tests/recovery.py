"""Contribution retries preserve verification and publication boundaries."""

import argparse
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('contribute', ROOT / 'krabb-loops/lib/contribute.py')
MODULE = importlib.util.module_from_spec(SPEC)

SPEC.loader.exec_module(MODULE)


class Recovery(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()

        self.addCleanup(self.directory.cleanup)

        self.root = Path(self.directory.name)
        self.loop = object.__new__(MODULE.Contribution)
        self.loop.repo = 'owner/repo'
        self.loop.base = 'main'
        self.loop.branch = 'feature'
        self.loop.sequence = 0
        self.loop.attempt = 0
        self.loop.args = argparse.Namespace(max_repairs=0, ci_timeout=1)
        self.loop.state = self.root / '.git/evidence'

        self.git('init', '--initial-branch=feature')
        self.git('config', 'user.name', 'Fixture')
        self.git('config', 'user.email', 'fixture@example.invalid')
        self.git('config', 'commit.gpgsign', 'false')
        self.git('config', 'core.hooksPath', str(self.root / '.git/hooks'))
        (self.root / 'file').write_text('initial')
        self.git('add', '.')
        self.git('commit', '-m', 'chore: initial')

        self.loop.sha = self.git('rev-parse', 'HEAD')

        self.loop.state.mkdir()

    def git(self, *args):
        return subprocess.run(['git', *args], cwd=self.root, capture_output=True,
                              text=True, check=True).stdout.strip()

    def execute(self):
        loop = self.loop
        loop.prepare = Mock()
        loop.describe = Mock(return_value=('feat: example', 'body', ''))
        loop.wait = Mock(return_value=None)
        loop.repair = Mock()

        return loop

    def test_staging_preserves_snapshot_for_add_delete_rename_binary_and_symlink(self):
        (self.root / 'file').rename(self.root / 'renamed')
        (self.root / 'binary').write_bytes(b'hello\0\xff')
        (self.root / 'link').symlink_to('missing')
        (self.root / 'renamed').chmod(0o755)

        with patch.object(MODULE, 'ROOT', self.root):
            before = MODULE.fingerprint()

            self.assertEqual(self.git('diff', '--cached'), '')

            self.git('add', '--all')

            self.assertEqual(MODULE.fingerprint(), before)

            (self.root / 'binary').write_bytes(b'different\0\xff')

            self.assertNotEqual(MODULE.fingerprint(), before)

    def test_snapshot_detects_modes_and_symlink_targets(self):
        with patch.object(MODULE, 'ROOT', self.root):
            before = MODULE.fingerprint()

            (self.root / 'file').chmod(0o755)

            self.assertNotEqual(MODULE.fingerprint(), before)

            (self.root / 'file').unlink()
            (self.root / 'file').symlink_to('one')

            before = MODULE.fingerprint()

            (self.root / 'file').unlink()
            (self.root / 'file').symlink_to('two')

            self.assertNotEqual(MODULE.fingerprint(), before)

    def test_directory_replacement_does_not_follow_old_tracked_children(self):
        folder = self.root / 'folder'

        folder.mkdir()
        (folder / 'child').write_text('tracked')
        self.git('add', '--all')
        (folder / 'child').unlink()
        folder.rmdir()
        folder.symlink_to('.')

        with patch.object(MODULE, 'ROOT', self.root):
            before = MODULE.fingerprint()

            self.git('add', '--all')

            self.assertEqual(MODULE.fingerprint(), before)

    def test_nested_repository_snapshot_survives_staging_and_detects_edits(self):
        nested = self.root / 'nested'

        nested.mkdir()
        self.git('-C', str(nested), 'init')
        self.git('-C', str(nested), 'config', 'user.name', 'Fixture')
        self.git('-C', str(nested), 'config', 'user.email', 'fixture@example.invalid')
        (nested / 'child').write_text('initial')
        self.git('-C', str(nested), 'add', '.')
        self.git('-C', str(nested), '-c', 'commit.gpgsign=false', 'commit', '-m', 'initial')

        with patch.object(MODULE, 'ROOT', self.root):
            before = MODULE.fingerprint()

            self.git('add', '--all')

            self.assertEqual(MODULE.fingerprint(), before)

            (nested / 'child').write_text('changed')

            self.assertNotEqual(MODULE.fingerprint(), before)

    def test_verification_changes_are_reverified(self):
        loop = self.loop
        loop.repair = Mock()

        def verify(*args, **kwargs):
            (self.root / 'file').write_text('formatted')

            return Mock(returncode=0)

        with patch.object(MODULE, 'ROOT', self.root):
            with patch.object(MODULE.subprocess, 'run', side_effect=verify) as command:
                with patch.object(loop, 'stable'), patch.object(
                        MODULE, 'fingerprint', side_effect=['old', 'new', 'new', 'new']):
                    self.assertEqual(loop.verify(), 'new')

            self.assertEqual(command.call_count, 2)
            loop.repair.assert_not_called()

    def test_actual_change_during_staging_reverifies_before_any_commit(self):
        loop = self.execute()
        loop.verify = Mock(side_effect=lambda: MODULE.fingerprint())
        original = MODULE.git
        staged = 0

        def git(*args):
            nonlocal staged

            result = original(*args)

            if args == ('add', '--all'):
                staged += 1

                if staged == 1:
                    (self.root / 'file').write_text('changed while staging')
            elif args == ('write-tree',) and staged == 2:
                # Stop after the second staging boundary; no remote or agent calls.
                raise RuntimeError('verified staging reached')

            return result

        with patch.object(MODULE, 'ROOT', self.root), patch.object(MODULE, 'git', side_effect=git):
            with self.assertRaisesRegex(RuntimeError, 'verified staging reached'):
                loop.execute()

        self.assertEqual(loop.verify.call_count, 2)
        self.assertEqual(self.git('rev-parse', 'HEAD'), loop.sha)
        loop.repair.assert_not_called()
        loop.wait.assert_not_called()

    def test_hook_failure_is_repaired_then_verified_before_retry(self):
        self.hook('echo broken-hook >&2\nexit 1\n')
        (self.root / 'file').write_text('feature')

        loop = self.execute()
        events = []
        loop.verify = lambda: events.append('verify') or MODULE.fingerprint()
        original = loop.publish

        def publish(*args):
            if loop.attempt:
                events.append('publish')

                return '42'

            return original(*args)

        def repair(evidence):
            self.assertIn('broken-hook', evidence.read_text())

            events.append('repair')

        loop.publish = publish
        loop.repair = repair

        with patch.object(MODULE, 'ROOT', self.root):
            loop.execute()

        self.assertEqual(events, ['verify', 'repair', 'verify', 'publish'])
        loop.wait.assert_called_once_with('42')

    def hook(self, body):
        path = self.root / '.git/hooks/pre-commit'

        path.write_text('#!/bin/sh\n' + body)
        path.chmod(0o755)

    def test_hook_edits_require_verification_even_when_commit_succeeds(self):
        for exit_code in (0, 1):
            with self.subTest(exit_code=exit_code):
                self.hook(f'echo formatted-{exit_code} > file\nexit {exit_code}\n')
                (self.root / 'file').write_text(f'feature-{exit_code}')

                with patch.object(MODULE, 'ROOT', self.root):
                    with self.assertRaises(MODULE.Reverify) as failure:
                        self.loop.publish('feat: example', 'body', '', MODULE.fingerprint(), False)

                self.assertIsNone(failure.exception.evidence)
                self.assertEqual(self.loop.sha, self.git('rev-parse', 'HEAD'))

    def test_metadata_is_regenerated_with_validation_feedback(self):
        self.loop.agent = Mock(side_effect=[
            'not json', json.dumps({'title': 'feat: example', 'solution': ['Adds example.']}),
        ])

        with patch.object(self.loop, 'stable'), patch.object(MODULE, 'gh', return_value=[]):
            title, _, _ = self.loop.describe()

        self.assertEqual(title, 'feat: example')
        self.assertIn('previous response was invalid', self.loop.agent.call_args.args[0])

    def test_changes_while_waiting_for_ci_require_another_verified_publication(self):
        loop = self.execute()
        loop.verify = Mock(return_value='verified')
        loop.publish = Mock(return_value='42')
        loop.wait = Mock(side_effect=[MODULE.Reverify('working tree changed while CI ran'), None])

        with patch.object(MODULE, 'ROOT', self.root):
            loop.execute()

        self.assertEqual(loop.verify.call_count, 2)
        self.assertEqual(loop.publish.call_count, 2)
        self.assertEqual(loop.wait.call_count, 2)
        loop.repair.assert_not_called()

    def test_recovery_budget_is_shared_across_failure_phases(self):
        self.loop.args.max_repairs = 1

        with patch.object(MODULE, 'ROOT', self.root):
            self.loop.recover('verification changed content')

            with self.assertRaisesRegex(RuntimeError, 'repair limit'):
                self.loop.recover('commit hook changed content')

    def test_no_change_repair_honors_budget_and_displays_worker_explanation(self):
        self.loop.args.max_repairs = 1
        self.loop.agent = Mock(return_value='Missing credentials prevent the repair.')

        with patch.object(MODULE, 'ROOT', self.root), patch('builtins.print') as output:
            with self.assertRaisesRegex(RuntimeError, 'repair limit'):
                self.loop.recover('CI failed', self.root / 'ci.log')

        self.loop.agent.assert_called_once()

        self.assertTrue(any('Missing credentials prevent the repair.' in str(call)
                            for call in output.call_args_list))

        self.assertEqual(self.git('rev-parse', 'HEAD'), self.loop.sha)

    def test_guidance_failure_is_repaired_before_running_workspace_verification(self):
        guidance = self.root / 'AGENTS.md'

        guidance.write_text('stale path')

        events = []

        def verify(command, **kwargs):
            if 'agentskill' in command:
                events.append('guidance')
                kwargs['stdout'].write('guidance validation result\n')

                return Mock(returncode=int(guidance.read_text() == 'stale path'))

            events.append('make verify')

            return Mock(returncode=0)

        def repair(evidence):
            self.assertIn('guidance validation result', evidence.read_text())

            events.append('repair')
            guidance.write_text('current path')

        self.loop.repair = repair

        with patch.object(MODULE, 'ROOT', self.root), patch.object(self.loop, 'stable'):
            with patch.object(MODULE, 'fingerprint', return_value='snapshot'):
                with patch.object(MODULE.shutil, 'which', return_value='/bin/agentskill'):
                    with patch.object(MODULE.subprocess, 'run', side_effect=verify):
                        self.assertEqual(self.loop.verify(), 'snapshot')

        self.assertEqual(events, ['guidance', 'repair', 'guidance', 'make verify'])

    def test_external_head_changes_remain_blockers(self):
        loop = self.execute()
        loop.verify = Mock(side_effect=MODULE.Reverify('content changed'))
        loop.sha = 'external'
        loop.publish = Mock()

        with patch.object(MODULE, 'ROOT', self.root):
            with self.assertRaisesRegex(RuntimeError, 'HEAD changed'):
                loop.execute()

        loop.publish.assert_not_called()
        loop.repair.assert_not_called()


if __name__ == '__main__':
    unittest.main()
