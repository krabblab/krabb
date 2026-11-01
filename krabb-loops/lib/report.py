"""Validate untrusted reports and normalize the legacy review contract."""

import json
from pathlib import Path, PurePosixPath
import sys


CLEAR = 'Review is clean, so no findings will be listed.'
HEADER = 'Review is not clean, findings are:'

COMMON = {'severity', 'blocking', 'file', 'problem', 'evidence', 'impact', 'action',
          'scope', 'validation'}

EXTRA = {'rev': set(), 'sec': {'attacker', 'reachability', 'proven'},
         'test': {'contract', 'gap', 'regression', 'sensitivity', 'coverage_only'}}


def unique(pairs):
    result = {}

    for key, value in pairs:
        if key in result:
            raise ValueError(f'duplicate JSON key: {key}')

        result[key] = value

    return result


def text(value):
    if not isinstance(value, str) or not value.strip() or len(value) > 16000:
        raise ValueError('expected a nonempty bounded text field')

    if any(ord(char) < 32 and char not in '\n\t' for char in value):
        raise ValueError('control characters are not allowed')


def legacy(content):
    if content == CLEAR:
        return {'findings': [], 'limitations': []}

    lines = content.splitlines()

    if not lines or lines.pop(0) != HEADER:
        raise ValueError('expected JSON or a legacy review report')

    fields = ('Severity', 'Blocking', 'File', 'Problem', 'Evidence', 'Failure Mode',
              'Required Fix')

    findings = []

    while lines:
        if not lines[0]:
            lines.pop(0)
            continue

        values = []

        for field in fields:
            if not lines or not lines[0].startswith(field + ': '):
                raise ValueError('malformed legacy finding block')

            values.append(lines.pop(0)[len(field) + 2:])

        severity, blocking, file, problem, evidence, impact, action = values

        if blocking not in ('Yes', 'No'):
            raise ValueError('invalid legacy blocking status')

        findings.append(dict(severity=severity, blocking=blocking == 'Yes', file=file,
                             problem=problem, evidence=evidence, impact=impact, action=action,
                             scope='Legacy review: selected invocation scope',
                             validation='Worker must establish a focused regression check.'))

    if not findings:
        raise ValueError('empty legacy findings report')

    return {'findings': findings, 'limitations': []}


def validate(value, domain):
    if not isinstance(value, dict) or set(value) != {'findings', 'limitations'}:
        raise ValueError('report must contain exactly findings and limitations')

    findings = value['findings']
    limitations = value['limitations']

    if not isinstance(findings, list) or len(findings) > 100:
        raise ValueError('findings must be an array of at most 100 entries')

    if not isinstance(limitations, list) or len(limitations) > 100:
        raise ValueError('limitations must be an array of at most 100 entries')

    for limitation in limitations:
        text(limitation)

    seen = set()

    for finding in findings:
        if not isinstance(finding, dict) or set(finding) != COMMON | EXTRA[domain]:
            raise ValueError(f'incorrect {domain} finding fields')

        if finding['severity'] not in ('Critical', 'High', 'Medium'):
            raise ValueError('invalid severity')

        if type(finding['blocking']) is not bool:
            raise ValueError('blocking must be a boolean')

        booleans = {'blocking', 'proven', 'coverage_only'}

        for key, item in finding.items():
            if key not in booleans:
                text(item)

        path = PurePosixPath(finding['file'].split(':', 1)[0])

        if path.is_absolute() or '..' in path.parts or not path.parts:
            raise ValueError('file must be a repository-relative path')

        if any(part in ('.git', '.revloop', '.krabb', 'target') for part in path.parts):
            raise ValueError('generated state cannot be a finding target')

        identity = (finding['file'], finding['problem'])

        if identity in seen:
            raise ValueError('duplicate finding')

        seen.add(identity)

        if domain == 'sec':
            if type(finding['proven']) is not bool or not finding['proven']:
                raise ValueError('security findings require reachability; use limitations')

        if domain == 'test':
            if type(finding['coverage_only']) is not bool or finding['coverage_only']:
                raise ValueError('coverage-only work is not a behavioral test gap')

    return value


def main():
    domain, filename, destination = sys.argv[1:]
    source = Path(filename)

    if source.stat().st_size > 2 * 1024 * 1024:
        raise ValueError('report exceeds 2 MiB')

    content = source.read_text().strip()

    if domain == 'rev' and not content.startswith('{'):
        value = legacy(content)
    else:
        value = json.loads(content, object_pairs_hook=unique)

    value = validate(value, domain)

    Path(destination).write_text(json.dumps(value, indent=2) + '\n')

    blockers = sum(item['blocking'] for item in value['findings'])

    Path(destination + '.counts').write_text(
        f"{len(value['findings'])} {blockers} {len(value['limitations'])}\n")


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, TypeError, KeyError, RecursionError) as error:
        print(f'Invalid orchestrator report: {error}', file=sys.stderr)
        sys.exit(72)
