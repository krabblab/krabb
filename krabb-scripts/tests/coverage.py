"""Coverage gates must use current mappings and preserve genuine failures."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / 'coverage.sh'
PACKAGES = ['krabb', 'krabb-core', 'krabb-file', 'krabb-runtime', 'krabb-daemon']

CURRENT = '''pub fn value() -> u64 {
    99
}

#[test]
fn check() {
    assert_eq!(value(), 99);
}
'''


class Coverage(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='krabb coverage ')

        self.addCleanup(self.directory.cleanup)

        self.root = Path(self.directory.name)
        self.env = dict(os.environ)

        self.env.pop('LLVM_PROFILE_FILE', None)

        self.env.update(CARGO=shutil.which('cargo') or 'cargo', CARGO_NET_OFFLINE='true',
                        CARGO_BUILD_JOBS='2', CARGO_TARGET_DIR=str(self.root / 'target'),
                        CARGO_LLVM_COV_TARGET_DIR=str(self.root / 'coverage'),
                        KRABB_COVERAGE_PACKAGE_MIN='80', KRABB_COVERAGE_RUNTIME_MIN='80')

    def run_command(self, args):
        return subprocess.run(args, cwd=self.root, env=self.env, capture_output=True,
                              text=True, timeout=120)

    def workspace(self):
        if not shutil.which('cargo'):
            self.skipTest('cargo is required for the coverage mapping fixture')

        available = self.run_command(['cargo', 'llvm-cov', '--version'])

        if available.returncode:
            self.skipTest('cargo-llvm-cov is required for the coverage mapping fixture')

        members = ', '.join(f'"{package}"' for package in PACKAGES)

        (self.root / 'Cargo.toml').write_text(
            f'[workspace]\nmembers = [{members}]\nresolver = "3"\n')

        for package in PACKAGES:
            source = self.root / package / 'src'

            source.mkdir(parents=True)

            manifest = (f'[package]\nname = "{package}"\nversion = "0.1.0"\n'
                        'edition = "2024"\n')

            code = CURRENT

            if package == 'krabb-daemon':
                manifest += '[dependencies]\nkrabb-runtime = { path = "../krabb-runtime" }\n'

                code = '#[test]\nfn check() { assert_eq!(krabb_runtime::value(), 99); }\n'

            (source.parent / 'Cargo.toml').write_text(manifest)
            (source / 'lib.rs').write_text(code)

        locked = self.run_command(['cargo', 'generate-lockfile', '--offline'])

        self.assertEqual(locked.returncode, 0, locked.stderr)

        return self.root / 'krabb-runtime/src/lib.rs'

    def gate(self):
        return self.run_command(['bash', str(SCRIPT), 'platform'])

    def test_stale_dependency_mappings_cannot_fail_current_covered_code(self):
        runtime = self.workspace()
        old_lines = '\n'.join(f'    let _a{index} = {index};' for index in range(100))

        runtime.write_text('pub fn value() -> u64 {\n' + old_lines + '\n    99\n}\n')

        seeded = self.run_command(['cargo', 'llvm-cov', '--locked', '-p', 'krabb-daemon'])

        self.assertEqual(seeded.returncode, 0, seeded.stdout + seeded.stderr)

        runtime.write_text(CURRENT)

        result = self.gate()

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('All package tests and coverage gates passed.', result.stdout)

        runtime_report = result.stdout.split('[coverage] krabb-runtime\n')[1]
        runtime_report = runtime_report.split('[coverage] krabb-daemon\n')[0]

        totals = next(line.split() for line in runtime_report.splitlines()
                      if line.startswith('TOTAL'))

        self.assertEqual(totals[7:10], ['6', '0', '100.00%'])

    def test_current_uncovered_code_still_fails_the_threshold(self):
        runtime = self.workspace()
        uncovered = '\n'.join(f'    let _a{index} = {index};' for index in range(100))

        runtime.write_text(CURRENT + '\npub fn uncovered() {\n' + uncovered + '\n}\n')

        result = self.gate()

        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('krabb-runtime (exit 1)', result.stderr)
        self.assertIn('[coverage] krabb-daemon', result.stdout)
        self.assertNotIn('All package tests and coverage gates passed.', result.stdout)

    def test_test_failures_still_fail_the_gate(self):
        runtime = self.workspace()

        runtime.write_text(CURRENT.replace('assert_eq!(value(), 99)', 'assert_eq!(value(), 0)'))

        result = self.gate()

        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('test result: FAILED', result.stdout + result.stderr)
        self.assertIn('krabb-runtime (exit ', result.stderr)
        self.assertIn('[coverage] krabb-daemon', result.stdout)
        self.assertNotIn('All package tests and coverage gates passed.', result.stdout)

    def test_cleanup_failure_never_runs_or_accepts_a_report(self):
        cargo = self.root / 'cargo fixture'

        cargo.write_text('''#!/usr/bin/env bash
printf '%s\\n' "$*" >> calls
if [[ "$*" == 'llvm-cov clean --workspace --locked' ]]; then
    exit 37
fi
exit 0
''')

        cargo.chmod(0o700)

        self.env['CARGO'] = str(cargo)
        result = self.gate()

        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)

        self.assertEqual((self.root / 'calls').read_text().splitlines(),
                         ['llvm-cov clean --workspace --locked'] * len(PACKAGES))

        self.assertEqual(result.stderr.count('(exit 37)'), len(PACKAGES))
        self.assertNotIn('All package tests and coverage gates passed.', result.stdout)


if __name__ == '__main__':
    unittest.main(verbosity=2)
