"""Extract a Claude print-mode result without interpreting any response as code."""

import json
from pathlib import Path
import sys

try:
    source = Path(sys.argv[1])

    if source.stat().st_size > 4 * 1024 * 1024:
        raise ValueError('agent response exceeds 4 MiB')

    events = [json.loads(line) for line in source.read_text().splitlines() if line.strip()]

    if not events or any(not isinstance(event, dict) for event in events):
        raise ValueError('expected JSON event objects')

    results = [event for event in events if event.get('type') == 'result']

    if len(results) != 1 or events[-1] is not results[0]:
        raise ValueError('expected exactly one terminal result event')

    value = results[0]

    if not isinstance(value, dict) or value.get('type') != 'result':
        raise ValueError('expected a result object')

    if value.get('is_error') is not False or value.get('subtype') != 'success':
        raise ValueError('agent did not report successful execution')

    result = value.get('result')

    if not isinstance(result, str) or not result.strip():
        raise ValueError('missing final response')

    Path(sys.argv[2]).write_text(result)
except (OSError, ValueError) as error:
    print(f'Invalid agent response: {error}', file=sys.stderr)
    sys.exit(70)
