"""Contribution publication and completion contracts; no provider or GitHub calls."""

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


def status():
    return {
        'headRefOid': 'abc', 'state': 'OPEN', 'mergeable': 'MERGEABLE',
        'statusCheckRollup': [
            {'__typename': 'CheckRun', 'status': 'COMPLETED', 'conclusion': 'SUCCESS'},
            {'__typename': 'CheckRun', 'status': 'COMPLETED', 'conclusion': 'SKIPPED'},
        ],
    }, {'headSha': 'abc', 'status': 'completed', 'conclusion': 'success'}


class Contributions(unittest.TestCase):
    def test_metadata_preserves_template_and_rejects_scopes(self):
        template = (ROOT / '.github/PULL_REQUEST_TEMPLATE.md').read_text()

        title, body, label = MODULE.metadata(json.dumps({
            'title': 'feat: preserve rich messages',
            'solution': ['Preserves supported channel formatting.'],
        }), template)

        self.assertEqual(title, 'feat: preserve rich messages')
        self.assertEqual(label, 'enhancement')
        self.assertIn('- [x] **Feature**', body)
        self.assertIn("- Doesn't close any.", body)
        self.assertIn('<!-- Describe the solution', body)
        self.assertIn('`make verify` passed.', body)

        for invalid in ('feat(core): scoped', 'Feat: upper', 'feat: ' + 'a' * 50, 'feat: x\ny'):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                MODULE.metadata(json.dumps({'title': invalid, 'solution': ['Example.']}), template)

        with self.assertRaises(ValueError):
            MODULE.metadata(json.dumps({'title': title, 'solution': ['Example.']}), '')

    def test_github_remote_forms(self):
        for remote in ('git@github.com:krabblab/krabb.git',
                       'git@github.com:/krabblab/krabb.git',
                       'https://github.com/krabblab/krabb',
                       'https://github.com/krabblab/krabb.git/'):
            self.assertEqual(MODULE.repository(remote), 'krabblab/krabb')

        with self.assertRaises(ValueError):
            MODULE.repository('https://example.com/krabblab/krabb')

    def test_completion_requires_finished_workflow_and_all_checks(self):
        pr, workflow = status()

        self.assertEqual(MODULE.checks(pr, workflow, 'abc'), 'passed')
        self.assertEqual(MODULE.checks(pr, None, 'abc'), 'pending')

        workflow['status'] = 'in_progress'

        self.assertEqual(MODULE.checks(pr, workflow, 'abc'), 'pending')

        workflow['status'] = 'completed'
        pr['statusCheckRollup'][0]['status'] = 'IN_PROGRESS'

        self.assertEqual(MODULE.checks(pr, workflow, 'abc'), 'pending')

        pr['statusCheckRollup'] = []

        self.assertEqual(MODULE.checks(pr, workflow, 'abc'), 'pending')

    def test_red_cancelled_and_external_checks_do_not_pass(self):
        pr, workflow = status()

        for conclusion in ('failure', 'cancelled', 'timed_out', 'skipped', None):
            workflow['conclusion'] = conclusion

            self.assertEqual(MODULE.checks(pr, workflow, 'abc'), 'failed')

        workflow['conclusion'] = 'success'

        for conclusion in ('FAILURE', 'CANCELLED', 'TIMED_OUT', None):
            pr['statusCheckRollup'][0]['conclusion'] = conclusion

            self.assertEqual(MODULE.checks(pr, workflow, 'abc'), 'failed')

        pr['statusCheckRollup'] = [{'__typename': 'StatusContext', 'state': 'PENDING'}]

        self.assertEqual(MODULE.checks(pr, workflow, 'abc'), 'pending')

        pr['statusCheckRollup'][0]['state'] = 'ERROR'

        self.assertEqual(MODULE.checks(pr, workflow, 'abc'), 'failed')

    def test_old_head_and_conflicts_stop_completion(self):
        pr, workflow = status()

        with self.assertRaises(RuntimeError):
            MODULE.checks(pr, workflow, 'old')

        workflow['headSha'] = 'old'

        with self.assertRaises(RuntimeError):
            MODULE.checks(pr, workflow, 'abc')

        pr['mergeable'] = 'CONFLICTING'

        with self.assertRaises(RuntimeError):
            MODULE.checks(pr, workflow, 'abc')

    def contribution(self):
        loop = object.__new__(MODULE.Contribution)
        loop.repo = 'owner/repo'
        loop.base = 'main'
        loop.branch = 'feature'
        loop.sha = 'abc'
        loop.sequence = 0
        loop.args = argparse.Namespace(max_repairs=0, ci_timeout=1)

        return loop

    def test_red_ci_repairs_reverifies_commits_and_waits_again(self):
        loop = self.contribution()
        events = []
        loop.prepare = lambda: events.append('prepare')
        loop.verify = lambda: events.append('verify') or 'verified'
        loop.describe = lambda: ('feat: contribution', 'body', 'enhancement')
        loop.publish = lambda *args: events.append(('publish', args[-1])) or '42'
        loop.wait = Mock(side_effect=['failed.log', None])
        loop.repair = lambda evidence: events.append(('repair', evidence))

        loop.execute()

        self.assertEqual(events, ['prepare', 'verify', ('publish', False),
                                  ('repair', 'failed.log'), 'verify', ('publish', True)])

        self.assertEqual(loop.wait.call_count, 2)

    def test_no_change_repair_is_a_blocker(self):
        loop = self.contribution()
        loop.state = Path('/evidence')
        loop.agent = Mock()

        with patch.object(MODULE, 'fingerprint', return_value='same'):
            with self.assertRaisesRegex(RuntimeError, 'no repair'):
                loop.repair('failure.log')

    def test_publish_stops_before_staging_if_verification_is_stale(self):
        loop = self.contribution()
        loop.stable = Mock()

        with patch.object(MODULE, 'fingerprint', return_value='new'):
            with patch.object(MODULE, 'git') as git:
                with self.assertRaisesRegex(RuntimeError, 'since verification'):
                    loop.publish('feat: example', 'body', '', 'old', False)

                git.assert_not_called()

    def test_ci_wait_marks_ready_only_after_green(self):
        loop = self.contribution()
        loop.stable = Mock()
        pr, workflow = status()
        pr['url'] = 'https://github.com/owner/repo/pull/42'

        with patch.object(MODULE, 'gh', side_effect=[pr, [workflow]]):
            with patch.object(MODULE, 'git', return_value=''):
                with patch.object(MODULE, 'run') as run:
                    self.assertIsNone(loop.wait('42'))
                    run.assert_called_once_with('gh', 'pr', 'ready', '42', '--repo', 'owner/repo')

    def test_failed_ci_preserves_logs_without_marking_ready(self):
        loop = self.contribution()
        loop.stable = Mock()
        pr, workflow = status()
        pr['url'] = 'https://github.com/owner/repo/pull/42'

        workflow.update(conclusion='failure', databaseId=123)

        with tempfile.TemporaryDirectory() as directory:
            loop.state = Path(directory)

            with patch.object(MODULE, 'gh', side_effect=[pr, [workflow]]):
                result = Mock(stdout='failure', stderr='')

                with patch.object(MODULE, 'run', return_value=result) as run:
                    evidence = loop.wait('42')

                    self.assertIn('failure', evidence.read_text())
                    self.assertEqual(run.call_args.args[1:3], ('run', 'view'))

    def test_ci_timeout_cannot_mark_ready(self):
        loop = self.contribution()

        with patch.object(MODULE.time, 'monotonic', side_effect=[0, 2]):
            with patch.object(MODULE, 'run') as run:
                with self.assertRaisesRegex(RuntimeError, 'deadline'):
                    loop.wait('42')

                run.assert_not_called()

    def test_publish_creates_and_repairs_real_git_branch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / 'work'
            remote = Path(directory) / 'remote.git'

            root.mkdir()

            def local(*args):
                return subprocess.run(['git', *args], cwd=root, capture_output=True,
                                      text=True, check=True).stdout.strip()

            local('init', '--initial-branch=main')
            local('config', 'user.name', 'Fixture')
            local('config', 'user.email', 'fixture@example.invalid')
            local('config', 'commit.gpgsign', 'false')
            local('config', 'core.hooksPath', '/dev/null')
            (root / 'file').write_text('initial')
            local('add', '.')
            local('commit', '-m', 'chore: initial')
            local('init', '--bare', str(remote))
            local('remote', 'add', 'origin', str(remote))
            local('push', '-u', 'origin', 'main')
            local('switch', '-c', 'feature')
            (root / 'file').write_text('feature')

            loop = self.contribution()
            loop.sha = local('rev-parse', 'HEAD')
            loop.state = root / '.git/evidence'

            loop.state.mkdir()

            published = []
            original = MODULE.run

            def host(*args, **kwargs):
                if args[0] != 'gh':
                    return original(*args, **kwargs)

                command = args[1:3]

                if command == ('pr', 'list'):
                    value = [{'number': 42}] if published else []
                elif command == ('label', 'list'):
                    value = [{'name': 'enhancement'}]
                elif command == ('api', 'user'):
                    value = {'login': 'maintainer'}
                elif command == ('pr', 'view'):
                    value = {'number': 42, 'isDraft': True}
                elif command in (('pr', 'create'), ('pr', 'edit')):
                    published.append(args)

                    self.assertIn('--body-file', args)
                    self.assertIn('maintainer', args)
                    self.assertIn('enhancement', args)

                    value = None
                else:
                    self.fail(f'unexpected command: {args}')

                return Mock(stdout=json.dumps(value), returncode=0)

            with patch.object(MODULE, 'ROOT', root), patch.object(MODULE, 'run', side_effect=host):
                loop.publish('feat: example', 'body', 'enhancement', MODULE.fingerprint(), False)

                first = local('rev-parse', 'HEAD')

                self.assertEqual(local('log', '-1', '--format=%s'), 'feat: example')
                self.assertEqual(local('rev-parse', 'origin/feature'), first)
                self.assertIn('--draft', published[0])

                (root / 'file').write_text('repaired')

                loop.publish('feat: example', 'updated body', 'enhancement',
                             MODULE.fingerprint(), True)

                self.assertEqual(local('log', '-1', '--format=%s'),
                                 'fix: repair contribution checks')

                self.assertEqual(local('rev-parse', 'HEAD^'), first)
                self.assertEqual(local('rev-parse', 'origin/feature'), loop.sha)
                self.assertEqual(local('status', '--porcelain'), '')
                self.assertEqual(published[1][1:3], ('pr', 'edit'))


if __name__ == '__main__':
    unittest.main()
