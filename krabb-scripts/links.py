#!/usr/bin/env python3
"""Check that relative Markdown links and their anchors resolve.

links.py [ROOT]
    Read every Markdown file under ROOT (default: the repository) and report each
    relative link whose file or heading anchor is missing. External links are not
    fetched. Exits 1 when a link is broken.
"""

from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[1]
SKIPPED = {'.git', 'target', 'node_modules', '.krabb', '.revloop', '.cache', '.target'}
LINK = re.compile(r'!?\[[^\]]*\]\(([^)\s]+)\)')
HEADING = re.compile(r'#{1,6}\s+(.*)')
SCHEME = re.compile(r'[A-Za-z][A-Za-z0-9+.-]*:')


def documents(root):
    for path in sorted(root.rglob('*.md')):
        if not SKIPPED.intersection(path.relative_to(root).parts[:-1]):
            yield path


def prose(path):
    fenced = False

    for number, line in enumerate(path.read_text(encoding='utf-8').split('\n'), 1):
        if line.lstrip().startswith(('```', '~~~')):
            fenced = not fenced

            continue

        if not fenced:
            yield number, line


def slug(heading):
    text = re.sub(r'[^\w\- ]', '', heading.replace('`', '').strip().lower())

    return text.replace(' ', '-')


def anchors(path):
    found = set()
    counts = {}

    for _, line in prose(path):
        match = HEADING.match(line)

        if match:
            base = slug(match.group(1))
            count = counts.get(base, 0)
            counts[base] = count + 1

            found.add(base if count == 0 else f'{base}-{count}')

        found.update(re.findall(r'<a (?:name|id)="([^"]+)"', line))

    return found


def broken(root):
    cache = {}
    failures = []

    for path in documents(root):
        for number, line in prose(path):
            for target in LINK.findall(line):
                if SCHEME.match(target):
                    continue

                name, _, fragment = target.partition('#')
                resolved = (path.parent / name).resolve() if name else path
                place = f'{path.relative_to(root)}:{number}'

                if not resolved.exists():
                    failures.append(f'{place}: missing {target}')
                    continue

                if not fragment or resolved.suffix != '.md' or not resolved.is_file():
                    continue

                if resolved not in cache:
                    cache[resolved] = anchors(resolved)

                if fragment not in cache[resolved]:
                    failures.append(f'{place}: missing anchor {target}')

    return failures


def main(arguments):
    if arguments[:1] in (['-h'], ['--help']):
        print(__doc__.strip())
        return 0

    if len(arguments) > 1:
        print(__doc__.strip(), file=sys.stderr)
        return 2

    failures = broken(Path(arguments[0]).resolve() if arguments else ROOT)

    for failure in failures:
        print(failure, file=sys.stderr)

    if failures:
        return 1

    print('Markdown links resolve.')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
