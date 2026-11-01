"""Reject ambiguous or failed backend completion events."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


PARSER = Path(__file__).resolve().parents[1] / 'lib/response.py'
RESULT = dict(type='result', subtype='success', is_error=False, result='final evidence')


class Responses(unittest.TestCase):
    def extract(self, events):
        with tempfile.TemporaryDirectory(prefix='krabb response ') as directory:
            source = Path(directory) / 'stream.jsonl'
            output = Path(directory) / 'output.txt'

            source.write_text('\n'.join(json.dumps(event) for event in events))

            result = subprocess.run([sys.executable, str(PARSER), str(source), str(output)],
                                    capture_output=True, text=True)

            return result.returncode, output.read_text() if output.exists() else ''

    def test_stream_extracts_only_final_result(self):
        status, text = self.extract([{'type': 'assistant', 'message': 'tool activity'}, RESULT])

        self.assertEqual((status, text), (0, 'final evidence'))

    def test_error_or_ambiguous_completion_is_rejected(self):
        cases = [[{**RESULT, 'is_error': True}], [{**RESULT, 'subtype': 'error_max_turns'}],
                 [RESULT, RESULT], [RESULT, {'type': 'assistant'}], [], ['unexpected text']]

        for events in cases:
            with self.subTest(events=events):
                self.assertEqual(self.extract(events), (70, ''))


if __name__ == '__main__':
    unittest.main(verbosity=2)
