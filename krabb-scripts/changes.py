#!/usr/bin/env python3
"""Select the workspace packages that a change can affect, so CI tests only those.

changes.py [BASE]
    Compare BASE with HEAD and print `code` and `packages` as GitHub outputs (to
    $GITHUB_OUTPUT when it is set). `code=false` means only documentation or assets
    changed. `packages` is a JSON array of the changed packages and every package that
    depends on them; it is empty when every package must run. Without BASE, every package
    runs.
"""

import json
import os
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]

# Paths that no build, test, or workflow reads.
INERT_DIRECTORIES = ('krabb-assets/', 'krabb-docs/', '.github/ISSUE_TEMPLATE/')
INERT_ROOT = {'LICENSE', 'NOTICE', 'CITATION.cff'}
INERT_NAMES = {'README.md', 'AGENTS.md', 'AGENTS.reference.md', 'CHANGELOG.md'}

# The runtime's tests read every official plugin manifest.
MANIFEST = 'krabb-plugin.toml'
MANIFEST_READER = 'krabb-runtime'


def inert(path):
    name = path.rsplit('/', 1)[-1]

    if path.startswith(INERT_DIRECTORIES) or name in INERT_NAMES:
        return True

    return '/' not in path and (path in INERT_ROOT or path.endswith('.md'))


def owner(path, members):
    """Return the package whose directory holds PATH most closely, or None."""
    matches = [name for name, directory in members.items() if path.startswith(directory + '/')]
    return max(matches, key=lambda name: len(members[name]), default=None)


def select(changed, members, dependencies):
    """Return the packages to test, None when every package must run, or [] for no code.

    MEMBERS maps each package to its directory relative to the root, and DEPENDENCIES maps
    each package to the workspace packages it depends on, including dev-dependencies.
    """
    selected = set()

    for path in changed:
        if inert(path):
            continue

        package = owner(path, members)

        if package is None:
            return None

        selected.add(package)

        if path.startswith('krabb-plugins/') and path.endswith('/' + MANIFEST):
            selected.add(MANIFEST_READER)

    dependents = {name: set() for name in members}

    for name, needs in dependencies.items():
        for need in needs:
            dependents.setdefault(need, set()).add(name)

    pending = list(selected)

    while pending:
        for dependent in dependents.get(pending.pop(), ()):
            if dependent not in selected:
                selected.add(dependent)
                pending.append(dependent)

    return sorted(selected)


def workspace():
    output = subprocess.run(
        ['cargo', 'metadata', '--format-version', '1', '--no-deps', '--locked'],
        cwd=ROOT, check=True, capture_output=True, text=True,
    ).stdout

    packages = json.loads(output)['packages']
    members = {
        package['name']: Path(package['manifest_path']).parent.relative_to(ROOT).as_posix()
        for package in packages
    }

    dependencies = {
        package['name']: {
            dependency['name'] for dependency in package['dependencies']
            if dependency['name'] in members
        }
        for package in packages
    }

    return members, dependencies


def changes(base):
    output = subprocess.run(
        ['git', 'diff', '--name-only', '--no-renames', base, 'HEAD'],
        cwd=ROOT, check=True, capture_output=True, text=True,
    ).stdout

    return [line for line in output.splitlines() if line]


def main(arguments):
    if arguments[:1] in (['-h'], ['--help']):
        print(__doc__.strip())
        return 0

    if len(arguments) > 1:
        print(__doc__.strip(), file=sys.stderr)
        return 2

    packages = None

    if arguments:
        packages = select(changes(arguments[0]), *workspace())

    code = packages != []
    lines = [f'code={str(code).lower()}', f'packages={json.dumps(packages or [])}']
    target = os.environ.get('GITHUB_OUTPUT')

    if target:
        with open(target, 'a', encoding='utf-8') as output:
            output.write('\n'.join(lines) + '\n')

    print('\n'.join(lines))
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
