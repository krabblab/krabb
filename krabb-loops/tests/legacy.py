"""Behavioral baseline for the original revloop; uses no provider or Rust builds."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
CLEAR = 'Review is clean, so no findings will be listed.'

FINDING = '''Review is not clean, findings are:
Severity: High
Blocking: Yes
File: source.txt:1
Problem: fixture defect
Evidence: reproducible fixture mismatch
Failure Mode: invalid fixture result
Required Fix: correct the fixture
'''

FAKE = r'''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys

root = Path(os.environ['FIXTURE'])
name = Path(sys.argv[0]).name
args = sys.argv[1:]
if '--help' in args:
    print('--sandbox --ephemeral --output-last-message --ignore-rules')
    print('--restricted --safe-mode --tools --strict-mcp-config --no-session-persistence')
    print('--permission-mode --output-format --settings --effort')
    sys.exit(0)
if name == 'codex' and args[0] == 'sandbox':
    sys.exit(int(os.environ.get('SANDBOX_STATUS', '0')))
if name == 'codex' and 'mcp' in args and 'list' in args:
    print(json.dumps([{'name': 'fixture', 'enabled': True}]))
    sys.exit(0)
with (root / 'calls.jsonl').open('a') as output:
    output.write(json.dumps([name, args]) + '\n')
if name in ('codex', 'claude'):
    if name == 'codex':
        mode = args[args.index('--sandbox') + 1]
        destination = Path(args[args.index('--output-last-message') + 1])
    else:
        mode = 'workspace-write' if 'Write' in args[args.index('--tools') + 1] else 'read-only'
        destination = root / 'claude-result'

    prompt = sys.stdin.read()
    with (root / 'prompts.jsonl').open('a') as output:
        output.write(json.dumps([mode, prompt]) + '\n')
    if mode == 'workspace-write':
        if os.environ.get('WORKER_FIX'):
            Path('source.txt').write_text('corrected\n')
        destination.write_text('Worker finished.')
    else:
        counter = root / 'counter'
        index = int(counter.read_text()) if counter.exists() else 0
        counter.write_text(str(index + 1))
        reports = json.loads((root / 'reports.json').read_text())
        destination.write_text(reports[min(index, len(reports) - 1)])
    if name == 'claude':
        print(json.dumps({'type': 'result', 'subtype': 'success', 'is_error': False,
                          'result': destination.read_text()}))
    else:
        print('agent transcript marker')
    sys.exit(int(os.environ.get('AGENT_STATUS', '0')))
if name == 'make' and args and args[0] in ('verify', 'test'):
    limit = int(os.environ.get('TIMEOUT_' + args[0].upper(), '0'))
    counter = root / ('timeout-count-' + args[0])
    count = int(counter.read_text()) if counter.exists() else 0
    if count < limit:
        counter.write_text(str(count + 1))
        print('bounded timeout fixture', flush=True)
        import signal
        while True:
            signal.pause()
if name == 'make' and args == ['verify']:
    counter = root / 'verify-count'
    count = int(counter.read_text()) if counter.exists() else 0
    counter.write_text(str(count + 1))
    if count < int(os.environ.get('FAIL_VERIFY_ATTEMPTS', '0')):
        print('test result: FAILED. reproducible fixture assertion')
        sys.exit(1)
    marker = root / 'verified-once'
    if not marker.exists() and (os.environ.get('FAIL_VERIFY_ONCE') or
                                os.environ.get('TIMEOUT_VERIFY_ONCE')):
        marker.write_text('attempted')
        print('first verification attempt failed', flush=True)
        if os.environ.get('TIMEOUT_VERIFY_ONCE'):
            import signal
            while True:
                signal.pause()
        sys.exit(1)
if name == 'make' and args in (['test'], ['verify']) and os.environ.get('FAIL_TEST'):
    print('test result: FAILED')
    sys.exit(1)
'''


class LegacyContract(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='krabb loop baseline ')

        self.addCleanup(self.temp.cleanup)

        self.fixture = Path(self.temp.name) / 'observer'

        self.fixture.mkdir()

        self.root = Path(self.temp.name) / 'repository'

        self.root.mkdir()

        scripts = self.root / 'krabb-scripts'

        scripts.mkdir()
        shutil.copytree(ROOT / 'krabb-loops', self.root / 'krabb-loops')
        shutil.copy2(ROOT / 'krabb-scripts/revloop.sh', scripts / 'revloop.sh')
        (self.root / 'Cargo.toml').write_text('[workspace]\n')
        (self.root / 'Makefile').write_text('')

        (self.root / '.gitignore').write_text(
            '.revloop/\n.krabb/loops/\nbin/\n*.json*\ncounter\nclaude-result\n')

        (self.root / 'source.txt').write_text('original\n')
        self.git('init', '-q', '-b', 'main')
        self.git('add', '.')

        self.git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                 '-c', 'commit.gpgsign=false', 'commit', '-qm', 'fixture')

        bindir = self.root / 'bin'

        bindir.mkdir()

        for name in ('codex', 'claude', 'cargo', 'make', 'cargo-llvm-cov', 'agentskill',
                     'cargo-deny', 'cargo-audit'):
            path = bindir / name

            path.write_text(FAKE)
            path.chmod(0o700)

        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(('KRABB_', 'CODEX_'))}

        self.env.update(PATH=f'{bindir}:{os.environ["PATH"]}', FIXTURE=str(self.fixture),
                        KRABB_CODEX_HOME=str(self.root / 'private home'),
                        KRABB_REVLOOP_MAX_CYCLES='4', KRABB_REVLOOP_CLEAN_PASSES='2')

    def git(self, *args):
        return subprocess.run(['git', '-C', str(self.root), *args], check=True,
                              capture_output=True, text=True).stdout

    def run_loop(self, reports=None, *args):
        (self.fixture / 'reports.json').write_text(json.dumps(reports or [CLEAR]))

        entry = self.root / getattr(self, 'entry', 'krabb-scripts/revloop.sh')

        return subprocess.run(['bash', str(entry), *args],
                              cwd=self.root, env=self.env, text=True,
                              capture_output=True, timeout=20)

    def calls(self, name):
        path = self.fixture / 'calls.jsonl'

        return [args for command, args in map(json.loads, path.read_text().splitlines())
                if command == name]

    def test_clean_consecutive_fresh_invocations(self):
        result = self.run_loop()

        self.assertEqual(result.returncode, 0, result.stderr)

        calls = self.calls('codex')

        self.assertEqual(len(calls), 2)

        for args in calls:
            self.assertIn('--ephemeral', args)
            self.assertEqual(args[args.index('--model') + 1], 'gpt-6-astra')
            self.assertEqual(args[args.index('--sandbox') + 1], 'read-only')
            self.assertNotIn('resume', args)

    def test_blocker_worker_and_verification(self):
        result = self.run_loop([FINDING, CLEAR])

        self.assertEqual(result.returncode, 0, result.stderr)

        modes = [args[args.index('--sandbox') + 1] for args in self.calls('codex')]

        self.assertEqual(modes, ['read-only', 'workspace-write', 'read-only', 'read-only'])
        self.assertIn(['test'], self.calls('make'))

    def test_nonblocking_is_visible_without_worker(self):
        result = self.run_loop([FINDING.replace('Blocking: Yes', 'Blocking: No')])

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('non-blocking', result.stdout)
        self.assertTrue(all('workspace-write' not in args for args in self.calls('codex')))
        self.assertIn(['verify'], self.calls('make'))

    def test_invalid_reports_rejected(self):
        for report in ('unexpected content', FINDING.replace('Evidence:', 'Unknown:'),
                       CLEAR + '\nExplanation', FINDING + '\n```'):
            with self.subTest(report=report):
                result = self.run_loop([report])

                self.assertEqual(result.returncode, 72, result.stderr)

    def test_reports_normalize_legacy_output_to_json(self):
        result = self.run_loop([FINDING, CLEAR])

        self.assertEqual(result.returncode, 0, result.stderr)

        state = self.root / '.krabb/loops/rev'
        run = Path((state / 'latest-run.txt').read_text().strip())
        report = json.loads((run / 'cycle-01-orchestrator.json').read_text())

        self.assertEqual(report['findings'][0]['problem'], 'fixture defect')
        self.assertEqual((run / 'cycle-01-orchestrator.response').read_text(), FINDING)

        latest = json.loads((run / 'latest-orchestrator.json').read_text())

        self.assertEqual(latest, {'findings': [], 'limitations': []})
        self.assertTrue((run / 'cycle-01-orchestrator-prompt.md').is_file())
        self.assertTrue((run / 'latest-worker.md').is_file())
        self.assertFalse(list(run.glob('*.txt.json')))

        prompts = [json.loads(line)[1] for line in
                   (self.fixture / 'prompts.jsonl').read_text().splitlines()]

        self.assertIn('cycle-01-orchestrator.json', prompts[1])
        self.assertTrue(all('# Idiomatic Rust' in prompt for prompt in prompts))

    def test_invalid_response_is_preserved_without_publishing_json(self):
        result = self.run_loop(['malformed response'])

        self.assertEqual(result.returncode, 72, result.stderr)

        state = self.root / '.krabb/loops/rev'
        run = Path((state / 'latest-run.txt').read_text().strip())

        self.assertEqual((run / 'cycle-01-orchestrator.response').read_text(),
                         'malformed response')

        self.assertFalse((run / 'cycle-01-orchestrator.json').exists())
        self.assertFalse((run / 'latest-orchestrator.json').exists())

    def test_cycle_exhaustion(self):
        self.env['KRABB_REVLOOP_MAX_CYCLES'] = '1'

        self.assertEqual(self.run_loop([FINDING]).returncode, 73)

    def test_default_runs_until_clean_without_cycle_cap(self):
        self.env.pop('KRABB_REVLOOP_MAX_CYCLES')

        result = self.run_loop()

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('Maximum cycles: unlimited.', result.stdout)
        self.assertIn(['verify'], self.calls('make'))

    def test_explicit_unlimited_cycles(self):
        self.env['KRABB_REVLOOP_MAX_CYCLES'] = '0'
        result = self.run_loop([FINDING, CLEAR])

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('Maximum cycles: unlimited.', result.stdout)

    def test_agent_failure(self):
        self.env['AGENT_STATUS'] = '1'

        self.assertEqual(self.run_loop().returncode, 70)

    def test_scope_and_unrelated_changes_preserved(self):
        (self.root / 'unrelated.txt').write_text('user work\n')

        result = self.run_loop()

        self.assertIn('uncommitted working-tree changes', result.stdout)
        self.assertEqual((self.root / 'unrelated.txt').read_text(), 'user work\n')
        self.assertEqual(self.git('diff'), '')

    def test_global_scope(self):
        result = self.run_loop(None, '--global')

        self.assertIn('global repository-wide audit', result.stdout)

    def test_feature_scope(self):
        self.git('checkout', '-qb', 'feature')
        (self.root / 'source.txt').write_text('feature change\n')
        self.git('add', 'source.txt')

        self.git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                 '-c', 'commit.gpgsign=false', 'commit', '-qm', 'feature fixture')

        result = self.run_loop()

        self.assertIn("feature branch 'feature' against main", result.stdout)

    def test_empty_feature_branch_falls_back_to_latest_commit(self):
        self.git('checkout', '-qb', 'feature')

        result = self.run_loop()

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('latest commit because the feature branch has no changes', result.stdout)

    def test_latest_scope(self):
        result = self.run_loop()

        self.assertIn('latest commit on main', result.stdout)

    def test_verbose_and_clean(self):
        self.assertNotIn('agent transcript marker', self.run_loop().stdout)
        self.assertIn('agent transcript marker', self.run_loop(None, '--verbose').stdout)

    def test_configuration_and_fast(self):
        self.env.update(KRABB_REVLOOP_MODEL='fixture-model', KRABB_REVLOOP_REASONING='medium')

        self.assertEqual(self.run_loop(None, '--fast').returncode, 0)

        args = self.calls('codex')[0]

        self.assertEqual(args[args.index('--model') + 1], 'fixture-model')
        self.assertIn('features.fast_mode=true', args)
        self.assertIn('model_reasoning_effort="medium"', args)

    def test_invalid_configuration(self):
        self.env['KRABB_REVLOOP_MAX_CYCLES'] = '-1'

        self.assertEqual(self.run_loop().returncode, 2)

    def test_clean_start_requires_final_verification(self):
        self.assertEqual(self.run_loop().returncode, 0)
        self.assertIn(['verify'], self.calls('make'))

    def test_worker_requires_final_verification_before_success(self):
        self.assertEqual(self.run_loop([FINDING, CLEAR]).returncode, 0)
        self.assertIn(['verify'], self.calls('make'))

    def test_failed_focused_verification_cannot_converge(self):
        self.env['FAIL_TEST'] = '1'

        self.assertEqual(self.run_loop([FINDING, CLEAR]).returncode, 73)
        self.assertEqual(sum('workspace-write' in args for args in self.calls('codex')), 4)

    def test_shared_settings_and_specific_precedence(self):
        self.env.update(KRABB_LOOP_MODEL='shared', KRABB_REVLOOP_MODEL='specific',
                        KRABB_LOOP_MAX_CYCLES='1', KRABB_REVLOOP_MAX_CYCLES='4')

        self.assertEqual(self.run_loop().returncode, 0)

        args = self.calls('codex')[0]

        self.assertEqual(args[args.index('--model') + 1], 'specific')

    def test_unsupported_backend(self):
        result = self.run_loop(None, '--agent', 'gemini')

        self.assertEqual(result.returncode, 2)
        self.assertIn('Unsupported agent', result.stderr)

    def test_claude_restricted_roles(self):
        self.env['KRABB_LOOP_AGENT'] = 'claude'
        result = self.run_loop([FINDING, CLEAR])

        self.assertEqual(result.returncode, 0, result.stderr)

        calls = self.calls('claude')

        self.assertEqual(len(calls), 4)

        for index, args in enumerate(calls):
            self.assertIn('--restricted', args)
            self.assertIn('--safe-mode', args)
            self.assertIn('--no-session-persistence', args)
            self.assertIn('--strict-mcp-config', args)
            self.assertEqual(args[args.index('--model') + 1], 'claude-fable-5-1')

            tools = args[args.index('--tools') + 1]

            self.assertNotIn('Bash', tools)
            self.assertEqual('Write' in tools, index == 1)

    def test_claude_streams_use_jsonl_and_reports_use_json(self):
        self.env['KRABB_LOOP_AGENT'] = 'claude'
        result = self.run_loop()

        self.assertEqual(result.returncode, 0, result.stderr)

        state = self.root / '.krabb/loops/rev'
        run = Path((state / 'latest-run.txt').read_text().strip())

        events = [json.loads(line) for line in
                  (run / 'cycle-01-orchestrator.jsonl').read_text().splitlines()]

        self.assertEqual(events[-1]['type'], 'result')

        self.assertEqual(json.loads((run / 'latest-orchestrator.json').read_text()),
                         {'findings': [], 'limitations': []})

    def test_claude_rejects_fast(self):
        result = self.run_loop(None, '--agent', 'claude', '--fast')

        self.assertEqual(result.returncode, 2)
        self.assertIn('does not support', result.stderr)

    def test_cli_agent_overrides_environment(self):
        self.env['KRABB_LOOP_AGENT'] = 'claude'

        self.assertEqual(self.run_loop(None, '--agent', 'codex').returncode, 0)
        self.assertEqual(len(self.calls('codex')), 2)

    def test_state_private_ignored_and_outcome_recorded(self):
        self.assertEqual(self.run_loop().returncode, 0)

        state = self.root / '.krabb/loops/rev'
        run = Path((state / 'latest-run.txt').read_text().strip())

        self.assertEqual(run.stat().st_mode & 0o777, 0o700)
        self.assertEqual((run / 'outcome.txt').stat().st_mode & 0o777, 0o600)
        self.assertIn('exit=0', (run / 'outcome.txt').read_text())
        self.assertNotIn('.krabb', self.git('status', '--porcelain'))

    def test_repeated_product_failures_keep_receiving_repairs(self):
        self.env['FAIL_VERIFY_ATTEMPTS'] = '2'
        result = self.run_loop()

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sum('workspace-write' in args for args in self.calls('codex')), 2)
        self.assertEqual(self.calls('make').count(['verify']), 3)

    def test_final_product_failure_recovery(self):
        self.env['FAIL_VERIFY_ONCE'] = '1'
        result = self.run_loop()

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sum('workspace-write' in args for args in self.calls('codex')), 1)
        self.assertEqual(self.calls('make').count(['verify']), 2)

    def test_verification_timeout_retry_preserves_diagnostics(self):
        self.env.update(TIMEOUT_VERIFY_ONCE='1', KRABB_REVLOOP_VERIFICATION_TIMEOUT='1s')

        result = self.run_loop()

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls('make').count(['verify']), 2)
        self.assertTrue(all('workspace-write' not in args for args in self.calls('codex')))

        attempts = list((self.root / '.krabb/loops/rev').glob('*/*.attempt-*'))

        self.assertEqual(len(attempts), 1)
        self.assertIn('first verification attempt failed', attempts[0].read_text())

    def test_codex_disables_integrations_and_external_write_roots(self):
        self.assertEqual(self.run_loop().returncode, 0)

        args = self.calls('codex')[0]

        self.assertIn('mcp_servers."fixture".enabled=false', args)
        self.assertIn('sandbox_workspace_write.writable_roots=[]', args)

        for feature in ('hooks', 'apps', 'plugins', 'multi_agent', 'browser_use'):
            index = args.index(feature)

            self.assertEqual(args[index - 1], '--disable')

    @unittest.skipUnless(os.environ.get('KRABB_TEST_CODEX'),
                         'set KRABB_TEST_CODEX to test the installed sandbox without model calls')
    def test_native_codex_reads_repository_and_denies_writes(self):
        self.env['REAL_CODEX'] = os.environ['KRABB_TEST_CODEX']
        self.env['KRABB_LOOP_OUTPUT'] = 'verbose'
        self.env['KRABB_CODEX_HOME'] = str(self.fixture / 'codex-home')

        (self.fixture / 'codex-home').mkdir()

        source = (self.root / 'bin/codex').read_text()

        source = source.replace("    sys.exit(int(os.environ.get('SANDBOX_STATUS', '0')))",
                                "    os.execv(os.environ['REAL_CODEX'], "
                                "[os.environ['REAL_CODEX'], *args])")

        probe = r"""from pathlib import Path
import errno
assert Path('source.txt').read_text() == 'original\n'
for path in [Path('source.txt'), Path('../observer/outside.txt')]:
    try:
        path.write_text('forbidden')
    except OSError as error:
        assert error.errno in (errno.EACCES, errno.EPERM, errno.EROFS), error
    else:
        raise AssertionError(f'sandbox allowed mutation: {path}')
print('native sandbox read and write-denial checks passed')
"""

        insertion = ("    import subprocess\n"
                     "    subprocess.run([os.environ['REAL_CODEX'], 'sandbox', "
                     "'--config', 'sandbox_mode=\"read-only\"', '--', sys.executable, "
                     f"'-c', {probe!r}], check=True, timeout=15)\n")

        source = source.replace("    prompt = sys.stdin.read()",
                                insertion + "    prompt = sys.stdin.read()")

        (self.root / 'bin/codex').write_text(source)

        result = self.run_loop()

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((self.root / 'source.txt').read_text(), 'original\n')
        self.assertFalse((self.fixture / 'outside.txt').exists())

        log = next((self.root / '.krabb/loops/rev').glob('*/cycle-01-orchestrator.log'))

        self.assertIn('native sandbox read and write-denial checks passed', log.read_text())

    def test_codex_sandbox_failure_stops_before_model_call(self):
        self.env['SANDBOX_STATUS'] = '1'
        result = self.run_loop()

        self.assertEqual(result.returncode, 70, result.stderr)
        self.assertFalse((self.fixture / 'calls.jsonl').exists())

        logs = list((self.root / '.krabb/loops/rev').glob('*/cycle-*-orchestrator.log'))

        self.assertEqual(len(logs), 1)
        self.assertIn('sandbox preflight failed; no agent started', logs[0].read_text())

    def test_readonly_guard_blocks_agent_repository_write(self):
        self.env['KRABB_LOOP_AGENT'] = 'claude'
        agent = self.root / 'bin/claude'
        source = agent.read_text()

        source = source.replace("    prompt = sys.stdin.read()",
                                "    Path('source.txt').write_text('unexpected mutation')\n"
                                "    prompt = sys.stdin.read()")

        agent.write_text(source)

        result = self.run_loop()

        self.assertEqual(result.returncode, 70)
        self.assertEqual((self.root / 'source.txt').read_text(), 'original\n')

    def test_worker_changes_preserve_unrelated_staged_and_untracked_work(self):
        (self.root / 'source.txt').write_text('defect\n')
        (self.root / 'staged.txt').write_text('user staged content\n')
        (self.root / 'untracked.txt').write_text('user untracked content\n')
        self.git('add', 'staged.txt')

        staged = self.git('diff', '--cached')
        head = self.git('rev-parse', 'HEAD')
        self.env['WORKER_FIX'] = '1'
        result = self.run_loop([FINDING, CLEAR])

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / 'source.txt').read_text(), 'corrected\n')
        self.assertEqual((self.root / 'untracked.txt').read_text(), 'user untracked content\n')
        self.assertEqual(self.git('diff', '--cached'), staged)
        self.assertEqual(self.git('rev-parse', 'HEAD'), head)

    def test_latest_pointer_replaces_symlink_without_touching_target(self):
        state = self.root / '.krabb/loops/rev'

        state.mkdir(parents=True)

        outside = self.fixture / 'unrelated.txt'

        outside.write_text('preserve me')
        (state / 'latest-run.txt').symlink_to(outside)

        result = self.run_loop()

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(outside.read_text(), 'preserve me')
        self.assertFalse((state / 'latest-run.txt').is_symlink())

    def test_multiple_recovery_workers_keep_distinct_diagnostics(self):
        self.env.update(TIMEOUT_VERIFY='2', TIMEOUT_TEST='2',
                        KRABB_REVLOOP_VERIFICATION_TIMEOUT='1s')

        result = self.run_loop()

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sum('workspace-write' in args for args in self.calls('codex')), 2)

        state = self.root / '.krabb/loops/rev'
        run = Path((state / 'latest-run.txt').read_text().strip())

        reports = [path for path in run.glob('*-timeout-recovery-worker-*.md')
                   if not path.name.endswith('-prompt.md')]

        self.assertEqual(len(reports), 2)

        for report in reports:
            self.assertTrue(report.with_suffix('.log').exists())

    def test_symlinked_state_rejected(self):
        outside = self.root / 'outside'

        outside.mkdir()
        (self.root / '.krabb').symlink_to(outside, target_is_directory=True)

        result = self.run_loop()

        self.assertEqual(result.returncode, 74)
        self.assertEqual(list(outside.iterdir()), [])


if __name__ == '__main__':
    unittest.main(verbosity=2)
