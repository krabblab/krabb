#!/usr/bin/env python3
"""Bump Krabb and plugin versions; a merged bump is what releases them.

With no COMPONENT, bump every component whose sources or locked dependencies changed since
its current version was set. COMPONENT is `krabb` or a plugin ID. A stable bump also turns
the changelog's `Unreleased` section into the new version's section.

Without --version, a stable bump promotes a pre-release to its version, or raises the patch
version; --unstable bumps to the next `-rc.N` of that version. Always review the plan
before confirming it: only a person may approve a release.
"""

import argparse
import datetime
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib


ROOT = Path(__file__).resolve().parents[1]
KRABB = 'krabb'
KRABB_PACKAGES = ['krabb', 'krabb-daemon']
VERSION = re.compile(r'^(\d+)\.(\d+)\.(\d+)(?:-rc\.(\d+))?$')
HEADING = re.compile(r'^## \[(?P<version>[^\]]+)\](?P<date>.*)$', re.MULTILINE)
PROTOCOL = re.compile(r'pub const CURRENT: Self = Self \{ major: (\d+), minor: (\d+) \};')
CORE = 'krabb-core/src'
TYPES = f'{CORE}/types.rs'
NORMAL = {None, 'build'}


class Failure(Exception):
    pass


class Component:
    """A releasable unit, the files that hold its version, and the packages it ships."""

    def __init__(self, name, version, files, packages, changelog):
        self.name = name
        self.version = version
        self.files = files
        self.packages = packages
        self.changelog = changelog


def components(root=ROOT):
    cargo = tomllib.loads((root / 'Cargo.toml').read_text())

    found = {KRABB: Component(
        KRABB,
        cargo['workspace']['package']['version'],
        [(root / 'Cargo.toml', 'workspace.package'), (root / 'CITATION.cff', None)],
        KRABB_PACKAGES,
        root / 'CHANGELOG.md',
    )}

    for path in sorted((root / 'krabb-plugins').rglob('krabb-plugin.toml')):
        if 'target' in path.relative_to(root).parts:
            continue

        manifest = tomllib.loads(path.read_text())
        crate = tomllib.loads((path.parent / 'Cargo.toml').read_text())['package']

        found[manifest['id']] = Component(
            manifest['id'],
            manifest['version'],
            [(path, ''), (path.parent / 'Cargo.toml', 'package')],
            [crate['name']],
            path.parent / 'CHANGELOG.md',
        )

    return found


def parse(version):
    match = VERSION.match(version)

    if not match:
        raise Failure(f'Version {version} is not X.Y.Z or X.Y.Z-rc.N.')

    major, minor, patch, candidate = match.groups()

    return (int(major), int(minor), int(patch)), int(candidate) if candidate else None


def order(version):
    base, candidate = parse(version)

    return base, (1, 0) if candidate is None else (0, candidate)


def following(current, unstable, requested=None):
    """The version after `current`, at `requested` when one is given."""

    base, candidate = parse(current)

    if requested is not None:
        target, requested_candidate = parse(requested)

        if requested_candidate is not None:
            raise Failure('Pass --version as X.Y.Z; use --unstable for a pre-release.')
    elif candidate is not None:
        target = base
    else:
        target = (base[0], base[1], base[2] + 1)

    text = '.'.join(map(str, target))

    if unstable:
        number = candidate + 1 if target == base and candidate is not None else 1
        text = f'{text}-rc.{number}'

    if order(text) <= order(current):
        raise Failure(f'Version {text} is not newer than {current}.')

    return text


def git(*arguments, root=ROOT, check=True):
    result = subprocess.run(['git', *arguments], cwd=root, capture_output=True, text=True,
                            check=False)

    if check and result.returncode != 0:
        raise Failure(f"git {' '.join(arguments)} failed: {result.stderr.strip()}")

    return result


def metadata(root=ROOT):
    """Cargo's package graph; reading it also brings Cargo.lock up to date offline."""

    result = subprocess.run(['cargo', 'metadata', '--format-version', '1', '--offline'],
                            cwd=root, capture_output=True, text=True, check=False)

    if result.returncode != 0:
        raise Failure(f'cargo metadata failed: {result.stderr.strip()}')

    return json.loads(result.stdout)


def closure(meta, names):
    """The package IDs a set of packages ships, through normal and build dependencies."""

    nodes = {node['id']: node for node in meta['resolve']['nodes']}
    pending = [package['id'] for package in meta['packages'] if package['name'] in names]
    seen = set()

    while pending:
        package = pending.pop()

        if package in seen:
            continue

        seen.add(package)

        for dependency in nodes[package]['deps']:
            if any(kind['kind'] in NORMAL for kind in dependency['dep_kinds']):
                pending.append(dependency['pkg'])

    return seen


def locked(text):
    """External locked packages; path crates are compared through their directories."""

    return {(package['name'], package['version'], package['source'])
            for package in tomllib.loads(text).get('package', []) if package.get('source')}


def workspace(commit, root=ROOT):
    """Whether the root Cargo.toml changed beyond the workspace version since `commit`."""

    before = git('show', f'{commit}:Cargo.toml', root=root, check=False)

    if before.returncode != 0:
        return True

    def settings(text):
        value = tomllib.loads(text)

        value.get('workspace', {}).get('package', {}).pop('version', None)

        return value

    return settings(before.stdout) != settings((root / 'Cargo.toml').read_text())


def base(component, root=ROOT):
    """The commit that set the component's current version, if it is committed."""

    path, _ = component.files[0]
    pattern = '^version = "' + component.version.replace('.', r'\.') + '"$'
    relative = path.relative_to(root).as_posix()
    result = git('log', '-1', '--format=%H', '-G', pattern, '--', relative, root=root)

    return result.stdout.strip() or None


def changed(component, meta, root=ROOT):
    commit = base(component, root)

    if commit is None:
        return False

    shipped = closure(meta, component.packages)
    packages = {package['id']: package for package in meta['packages']}

    local = [Path(packages[package]['manifest_path']).parent for package in shipped
             if packages[package]['source'] is None]

    paths = sorted({path.relative_to(root).as_posix() for path in local})

    if git('diff', '--quiet', commit, '--', *paths, root=root, check=False).returncode:
        return True

    if git('ls-files', '--others', '--exclude-standard', '--', *paths, root=root).stdout:
        return True

    if workspace(commit, root):
        return True

    before = git('show', f'{commit}:Cargo.lock', root=root, check=False)

    if before.returncode != 0:
        return True

    lock = root / 'Cargo.lock'
    moved = locked(before.stdout) ^ locked(lock.read_text())
    names = {packages[package]['name'] for package in shipped}

    return any(name in names for name, _, _ in moved)


def assign(path, section, version):
    """Sets the `version` key of one TOML section, or the CITATION.cff version."""

    text = path.read_text()

    if section is None:
        updated, count = re.subn(r'(?m)^version: .*$', f'version: {version}', text, count=1)
    else:
        lines = text.splitlines(keepends=True)
        current = ''
        count = 0

        for number, line in enumerate(lines):
            if line.startswith('['):
                current = line.strip().strip('[]')
            elif current == section and line.startswith('version = ') and not count:
                lines[number] = f'version = "{version}"\n'
                count = 1

        updated = ''.join(lines)

    if not count:
        raise Failure(f'{path} has no version to bump.')

    path.write_text(updated)


def roll(path, version, today):
    """Turns `Unreleased` into the version's section; returns whether notes exist."""

    if not path.is_file():
        return False

    text = path.read_text()
    headings = {match.group('version'): match for match in HEADING.finditer(text)}

    if version in headings:
        heading = headings[version]

        if not heading.group('date').strip():
            text = text[:heading.end()] + f' - {today}' + text[heading.end():]
    elif 'Unreleased' in headings:
        heading = headings['Unreleased']

        text = text[:heading.start()] + f'## [Unreleased]\n\n## [{version}] - {today}' + \
            text[heading.end():]
    else:
        return False

    path.write_text(text)

    return True


def plan(arguments, root=ROOT, meta=None):
    found = components(root)
    unknown = [name for name in arguments.components if name not in found]

    if unknown:
        raise Failure(f"Unknown component {', '.join(unknown)}; use krabb or a plugin ID.")

    if arguments.components:
        selected = [found[name] for name in arguments.components]
    else:
        meta = meta or metadata(root)
        selected = [component for component in found.values() if changed(component, meta, root)]

    return [(component, following(component.version, arguments.unstable, arguments.version))
            for component in selected]


def apply(bumps, root=ROOT, today=None):
    today = today or datetime.date.today().isoformat()

    for component, version in bumps:
        for path, section in component.files:
            assign(path, section, version)

        stable = parse(version)[1] is None

        if stable and not roll(component.changelog, version, today) and component.name == KRABB:
            print(f'[WARN] CHANGELOG.md has no notes for {version}, so its release uses the '
                  'default description.', file=sys.stderr)

    metadata(root)


def protocol(text):
    """The plugin protocol version a `types.rs` declares, such as `0.2`."""

    found = PROTOCOL.search(text or '')

    return found and f'{found[1]}.{found[2]}'


def unchanged_protocol(bumps, root=ROOT):
    """A warning when a Krabb bump changes `krabb-core` but keeps the plugin protocol."""

    krabb = next((component for component, _ in bumps if component.name == KRABB), None)
    commit = krabb and base(krabb, root)

    if not commit or not git('diff', '--quiet', commit, '--', CORE, root=root,
                             check=False).returncode:
        return None

    before = protocol(git('show', f'{commit}:{TYPES}', root=root, check=False).stdout)
    current = protocol((root / TYPES).read_text())

    if current is None:
        return f'{TYPES} no longer declares Protocol::CURRENT as bump.py expects; update bump.py.'

    if before != current:
        return None

    return (f'krabb-core changed since Krabb {krabb.version}, and the plugin protocol is still '
            f'{current}. Raise it if plugins rely on the change, as DEPRECATIONS.md describes.')


def confirmed(bumps, assume, root=ROOT):
    print('[INFO] Planned release bumps:')

    for component, version in bumps:
        print(f'  {component.name}: {component.version} -> {version}')

    warning = unchanged_protocol(bumps, root)

    if warning:
        print(f'[WARN] {warning}')

    sys.stdout.flush()

    if assume:
        return True

    if not sys.stdin.isatty():
        raise Failure('Confirm the bumps in a terminal, or pass --yes once a maintainer has '
                      'approved them.')

    return input('Apply these bumps? [y/N] ').strip().lower() in {'y', 'yes'}


def main(argv=None, root=ROOT):
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0],
                                     epilog=__doc__.split('\n\n', 1)[1])

    parser.add_argument('components', nargs='*', metavar='COMPONENT')
    parser.add_argument('--unstable', action='store_true', help='bump to a pre-release')
    parser.add_argument('--version', help='target X.Y.Z instead of the next one')
    parser.add_argument('--yes', action='store_true', help='apply an approved plan')

    arguments = parser.parse_args(argv)

    try:
        bumps = plan(arguments, root)

        if not bumps:
            print('[WARN] Nothing changed since the current versions, so nothing was bumped.',
                  file=sys.stderr)

            return 0

        if not confirmed(bumps, arguments.yes, root):
            print('[INFO] No version was changed.')
            return 1

        apply(bumps, root)
    except Failure as error:
        print(f'[ERROR] {error}', file=sys.stderr)
        return 2

    print(f'[INFO] Bumped {len(bumps)} component(s). Review the diff and open a pull request; '
          'merging it releases them.')

    return 0


if __name__ == '__main__':
    sys.exit(main())
