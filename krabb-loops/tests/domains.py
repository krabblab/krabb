"""Complete security/test loop fixtures: no authenticated agent or provider calls."""

import json
import unittest

import legacy


CLEAN = json.dumps({'findings': [], 'limitations': []})


def report(domain, **changes):
    finding = dict(severity='High', blocking=True, file='source.txt:1',
                   problem='fixture behavior is unsafe', evidence='local reproduction',
                   impact='incorrect fixture result', action='correct fixture behavior',
                   scope='selected source file', validation='assert failure then corrected output')

    if domain == 'sec':
        finding.update(attacker='local untrusted input', reachability='input reaches parser',
                       proven=True)
    else:
        finding.update(contract='reject invalid input', gap='invalid input is not tested',
                       regression='removed guard accepts invalid input',
                       sensitivity='remove guard temporarily; assertion must fail',
                       coverage_only=False)

    finding.update(changes)
    return json.dumps({'findings': [finding], 'limitations': []})


class Domains(unittest.TestCase):
    setUp = legacy.LegacyContract.setUp
    git = legacy.LegacyContract.git
    run_loop = legacy.LegacyContract.run_loop
    calls = legacy.LegacyContract.calls

    def run_domain(self, domain, reports):
        self.entry = f'krabb-loops/{domain}loop.sh'

        self.env.update(KRABB_LOOP_MAX_CYCLES='4', KRABB_LOOP_CLEAN_PASSES='2')
        return self.run_loop(reports)

    def test_security_remediation_and_independent_stability(self):
        result = self.run_domain('sec', [report('sec'), CLEAN])

        self.assertEqual(result.returncode, 0, result.stderr)

        modes = [args[args.index('--sandbox') + 1] for args in self.calls('codex')]

        self.assertEqual(modes, ['read-only', 'workspace-write', 'read-only', 'read-only'])
        self.assertIn(['audit'], self.calls('cargo'))
        self.assertIn(['deny', 'check'], self.calls('cargo'))
        self.assertIn(['verify'], self.calls('make'))

    def test_unproven_security_concern_never_reaches_worker(self):
        result = self.run_domain('sec', [report('sec', proven=False)])

        self.assertEqual(result.returncode, 72)
        self.assertEqual(len(self.calls('codex')), 1)

    def test_nonblocking_security_concern_does_not_modify(self):
        result = self.run_domain('sec', [report('sec', blocking=False)])

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(all('workspace-write' not in args for args in self.calls('codex')))

    def test_test_gap_requires_behavioral_value(self):
        result = self.run_domain('test', [report('test', coverage_only=True)])

        self.assertEqual(result.returncode, 72)
        self.assertEqual(len(self.calls('codex')), 1)

    def test_missing_contract_is_rejected(self):
        value = json.loads(report('test'))

        del value['findings'][0]['contract']

        result = self.run_domain('test', [json.dumps(value)])

        self.assertEqual(result.returncode, 72)
        self.assertEqual(len(self.calls('codex')), 1)

    def test_test_gap_worker_and_final_gates(self):
        result = self.run_domain('test', [report('test'), CLEAN])

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sum('workspace-write' in args for args in self.calls('codex')), 1)
        self.assertIn(['verify'], self.calls('make'))

        prompts = [json.loads(line)[1] for line in
                   (self.fixture / 'prompts.jsonl').read_text().splitlines()]

        self.assertTrue(all('# Idiomatic Rust' in prompt for prompt in prompts))
        self.assertIn('safe reversible mutation', prompts[1])
        self.assertIn('100% coverage', prompts[0])

    def test_confirmed_flaky_test_is_actionable(self):
        flaky = report('test', problem='parallel test races on a shared fixture',
                       evidence='same seed fails only with overlapping fixture writers',
                       contract='each test observes only its own state',
                       gap='shared filename leaks state across parallel tests',
                       regression='parallel worker overwrites another fixture',
                       sensitivity='controlled overlap reproduces; isolated fixtures remain stable')

        result = self.run_domain('test', [flaky, CLEAN])

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sum('workspace-write' in args for args in self.calls('codex')), 1)

        prompts = [json.loads(line)[1] for line in
                   (self.fixture / 'prompts.jsonl').read_text().splitlines()]

        self.assertIn('flaky test discovery and repair', prompts[0].lower())
        self.assertIn('repair confirmed flaky tests', prompts[1].lower())
        self.assertIn('automatic retries', prompts[1])

    def test_unresolved_investigation_never_counts_as_clean(self):
        limitation = json.dumps({'findings': [], 'limitations': ['Required local fixture missing']})
        result = self.run_domain('test', [limitation])

        self.assertEqual(result.returncode, 73)
        self.assertEqual(sum('workspace-write' in args for args in self.calls('codex')), 4)
        self.assertNotIn(['verify'], self.calls('make'))

    def test_security_findings_and_limitations_reach_worker_together(self):
        value = json.loads(report('sec'))

        value['limitations'] = [
            'Read-only reviewer could not run Rust regressions or make verify.',
            'Inspect scheduling intent and choose a compatible mitigation, not risk acceptance.',
            'Use a disposable Git fixture to investigate worktree cleanup hooks.']

        result = self.run_domain('sec', [json.dumps(value), CLEAN])

        self.assertEqual(result.returncode, 0, result.stderr)

        modes = [args[args.index('--sandbox') + 1] for args in self.calls('codex')]

        self.assertEqual(modes, ['read-only', 'workspace-write', 'read-only', 'read-only'])

        prompts = [json.loads(line)[1] for line in
                   (self.fixture / 'prompts.jsonl').read_text().splitlines()]

        self.assertTrue(all('# Idiomatic Rust' in prompt for prompt in prompts))
        self.assertIn('every limitation', prompts[1])
        self.assertIn('Latest worker evidence:', prompts[2])
        self.assertIn('latest-worker.md', prompts[2])
        self.assertIn('Focused verification passed', prompts[2])
        self.assertIn('Full verification passed', prompts[3])
        self.assertIn('latest-verification.log', prompts[3])
        self.assertNotIn('$review_file', prompts[1])
        self.assertIn(['verify'], self.calls('make'))

    def test_local_test_investigation_runs_without_claiming_a_proven_defect(self):
        value = json.dumps({'findings': [], 'limitations': [
            'Check a suspected order-dependent fixture with bounded reversed-order runs.']})

        result = self.run_domain('test', [value, CLEAN])

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sum('workspace-write' in args for args in self.calls('codex')), 1)
        self.assertEqual((self.root / 'source.txt').read_text(), 'original\n')
        self.assertIn(['verify'], self.calls('make'))

    def test_limitation_resets_clean_streak_and_requires_new_final_gates(self):
        value = json.dumps({'findings': [], 'limitations': ['Check the fixture cleanup contract.']})
        result = self.run_domain('sec', [CLEAN, value, CLEAN])

        self.assertEqual(result.returncode, 0, result.stderr)

        modes = [args[args.index('--sandbox') + 1] for args in self.calls('codex')]

        self.assertEqual(modes, ['read-only', 'read-only', 'workspace-write',
                                 'read-only', 'read-only'])

        self.assertEqual(self.calls('make').count(['verify']), 2)

    def test_review_loop_uses_the_same_investigation_handoff(self):
        value = json.dumps({'findings': [], 'limitations': ['Reproduce a cleanup race locally.']})
        result = self.run_domain('rev', [value, CLEAN])

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sum('workspace-write' in args for args in self.calls('codex')), 1)

    def test_clean_test_scope_converges_without_extra_tests(self):
        result = self.run_domain('test', [CLEAN])

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(self.calls('codex')), 2)
        self.assertTrue(all('workspace-write' not in args for args in self.calls('codex')))

    def test_duplicate_json_keys_rejected(self):
        value = '{"findings": [], "findings": [], "limitations": []}'

        self.assertEqual(self.run_domain('sec', [value]).returncode, 72)

    def test_report_cannot_target_artifacts(self):
        result = self.run_domain('sec', [report('sec', file='.krabb/loops/data')])

        self.assertEqual(result.returncode, 72)

    def test_report_text_is_not_executed(self):
        value = report('sec', blocking=False, evidence='$(touch INJECTED); `touch INJECTED`')
        result = self.run_domain('sec', [value])

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.root / 'INJECTED').exists())


if __name__ == '__main__':
    unittest.main(verbosity=2)
