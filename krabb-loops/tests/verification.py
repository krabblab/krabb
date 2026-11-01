"""Classification informs recovery without converting a failed gate to success."""

from pathlib import Path
import subprocess
import tempfile
import unittest


LIBRARY = Path(__file__).resolve().parents[1] / 'lib/verify.sh'


class Verification(unittest.TestCase):
    def test_failure_categories_preserve_distinct_evidence(self):
        cases = [('stalled command', 75, 'timeout'),
                 ('No space left on device', 1, 'resource'),
                 ('test result: FAILED. assertion failed', 101, 'test'),
                 ('Required final verification tool is missing: cargo-audit', 71, 'dependency'),
                 ('failed to acquire advisory database', 1, 'network'),
                 ('read-only file system', 1, 'environment'),
                 ('failed to parse manifest', 1, 'configuration'),
                 ('error: mismatched types', 101, 'product')]

        with tempfile.TemporaryDirectory(prefix='krabb failures ') as directory:
            log = Path(directory) / 'verification.log'

            for evidence, status, expected in cases:
                with self.subTest(expected=expected):
                    log.write_text(evidence)

                    result = subprocess.run(
                        ['bash', '-c', 'source "$1"; verification_failure_kind "$2" "$3"',
                         'fixture', str(LIBRARY), str(log), str(status)],
                        capture_output=True, text=True, check=True)

                    self.assertEqual(result.stdout.strip(), expected)


if __name__ == '__main__':
    unittest.main(verbosity=2)
