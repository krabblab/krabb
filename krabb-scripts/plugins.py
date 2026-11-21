#!/usr/bin/env python3
"""Plan and publish official plugin versions to the plugin index and its OCI registry.

plan INDEX
    Print the IDs of plugins whose manifest version is not in INDEX yet.

publish INDEX DIST REGISTRY SOURCE
    Push each unpublished plugin's archives from DIST to REGISTRY with oras, confirm that
    anonymous clients can pull them, and append the version to INDEX. Published versions
    are never changed or removed.
"""

import datetime
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib
import urllib.error
import urllib.parse
import urllib.request


ROOT = Path(__file__).resolve().parents[1]
SCHEMA = 1
ARTIFACT_TYPE = 'application/vnd.krabb.plugin.v1'
ARCHIVE_LIMIT = 64 * 1024 * 1024

TARGETS = {
    'x86_64-unknown-linux-gnu': 'tar.gz',
    'aarch64-unknown-linux-gnu': 'tar.gz',
    'x86_64-apple-darwin': 'tar.gz',
    'aarch64-apple-darwin': 'tar.gz',
    'x86_64-pc-windows-msvc': 'zip',
    'aarch64-pc-windows-msvc': 'zip',
}

MEDIA = {'tar.gz': 'application/gzip', 'zip': 'application/zip'}
ID = re.compile(r'^[a-z0-9-]+$')
VERSION = re.compile(r'^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$')
REGISTRY = re.compile(r'^[a-z0-9.:-]+(/[a-z0-9._-]+)+$')


class Failure(Exception):
    pass


class Redirects(urllib.request.HTTPRedirectHandler):
    """Stops at a blob redirect so the bearer token never reaches the storage host."""

    def redirect_request(self, *_):
        return None


def manifests(root=ROOT):
    """Every official plugin manifest by ID."""

    found = {}

    for path in sorted((root / 'krabb-plugins').rglob('krabb-plugin.toml')):
        if 'target' in path.relative_to(root).parts:
            continue

        manifest = tomllib.loads(path.read_text())
        plugin = manifest['id']

        if not ID.match(plugin):
            raise Failure(f'Plugin ID {plugin} is not valid.')

        if plugin in found:
            raise Failure(f'Plugin {plugin} has more than one manifest.')

        if not VERSION.match(manifest['version']):
            raise Failure(f"Plugin {plugin} has an invalid version: {manifest['version']}.")

        crate = tomllib.loads((path.parent / 'Cargo.toml').read_text()).get('package', {})
        crate = crate.get('version') if isinstance(crate, dict) else None

        if not isinstance(crate, str):
            raise Failure(f'Plugin {plugin} needs its own version in Cargo.toml.')

        if crate != manifest['version']:
            raise Failure(
                f"Plugin {plugin} is version {manifest['version']} in its manifest but {crate} "
                'in Cargo.toml; set both to the same version.'
            )

        found[plugin] = manifest

    return found


def load(index, registry=None):
    if not index.exists():
        if registry is None:
            return {'schema': SCHEMA, 'plugins': {}}

        return {'schema': SCHEMA, 'registry': registry, 'plugins': {}}

    value = json.loads(index.read_text())

    if value.get('schema') != SCHEMA:
        raise Failure(f"The plugin index uses schema {value.get('schema')}, not {SCHEMA}.")

    if registry is not None and value.get('registry') != registry:
        raise Failure(f"The plugin index publishes to {value.get('registry')}, not {registry}.")

    return value


def unpublished(value, found):
    published = value['plugins']

    return [plugin for plugin, manifest in found.items()
            if manifest['version'] not in {r['version'] for r in published.get(plugin, [])}]


def archives(dist, plugin, version):
    files = {}

    for target, extension in TARGETS.items():
        path = dist / f'krabb-{plugin}-v{version}-{target}.{extension}'

        if not path.is_file():
            raise Failure(f'Plugin {plugin} {version} is missing its {target} archive.')

        if path.stat().st_size > ARCHIVE_LIMIT:
            raise Failure(f'Plugin {plugin} {version} has a {target} archive over 64 MiB.')

        files[target] = path

    return files


def push(registry, source, plugin, version, files, dist):
    layers = [f'{path.name}:{MEDIA[TARGETS[target]]}' for target, path in files.items()]

    command = [
        'oras', 'push', f'{registry}:{plugin}-{version}',
        '--artifact-type', ARTIFACT_TYPE,
        '--annotation', f'org.opencontainers.image.source={source}',
        '--annotation', f'org.opencontainers.image.title={plugin}',
        '--annotation', f'org.opencontainers.image.version={version}',
        *layers,
    ]

    result = subprocess.run(command, cwd=dist, capture_output=True, text=True, check=False)

    if result.returncode != 0:
        raise Failure(f'Pushing {plugin} {version} failed: {result.stderr.strip()}')


def pullable(registry, digests):
    """Confirms that an anonymous client can download every blob, as Krabb does."""

    host, repository = registry.split('/', 1)

    query = urllib.parse.urlencode(
        {'scope': f'repository:{repository}:pull', 'service': host},
    )

    try:
        with urllib.request.urlopen(f'https://{host}/token?{query}', timeout=30) as response:
            token = json.loads(response.read(64 * 1024)).get('token', '')

        opener = urllib.request.build_opener(Redirects)

        for digest in digests:
            request = urllib.request.Request(
                f'https://{host}/v2/{repository}/blobs/sha256:{digest}',
                method='HEAD', headers={'Authorization': f'Bearer {token}'},
            )

            try:
                with opener.open(request, timeout=30):
                    pass
            except urllib.error.HTTPError as error:
                if error.code not in (301, 302, 303, 307, 308):
                    return False
    except (OSError, ValueError):
        return False

    return True


def digest(path):
    with path.open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def publish(index, dist, registry, source, root=ROOT, today=None):
    if not REGISTRY.match(registry):
        raise Failure(f'Registry {registry} is not a valid OCI repository.')

    value = load(index, registry)
    found = manifests(root)
    released = today or datetime.datetime.now(datetime.timezone.utc).date().isoformat()

    for plugin in unpublished(value, found):
        manifest = found[plugin]
        version = manifest['version']
        files = archives(dist, plugin, version)

        artifacts = {target: {'sha256': digest(path), 'size': path.stat().st_size}
                     for target, path in files.items()}

        push(registry, source, plugin, version, files, dist)

        if not pullable(registry, [artifact['sha256'] for artifact in artifacts.values()]):
            raise Failure(
                f'Anonymous clients cannot pull {plugin} {version} from {registry}. Make the '
                'package public in its GitHub settings, then run the Plugins workflow again.'
            )

        value['plugins'].setdefault(plugin, []).append({
            'version': version,
            'protocol': {'major': manifest['protocol']['major'],
                         'minor': manifest['protocol']['minor']},
            'released': released,
            'permissions': manifest.get('permissions', []),
            'secrets': manifest.get('secrets', []),
            'artifacts': artifacts,
        })

        value['plugins'] = dict(sorted(value['plugins'].items()))

        index.write_text(json.dumps(value, indent=2) + '\n')
        print(f'[INFO] Published {plugin} {version}.')


def main(arguments):
    try:
        match arguments:
            case ['plan', index]:
                print(' '.join(unpublished(load(Path(index)), manifests())))
            case ['publish', index, dist, registry, source]:
                publish(Path(index), Path(dist), registry, source)
            case _:
                print(__doc__.strip(), file=sys.stderr)
                return 2
    except Failure as error:
        print(f'[ERROR] {error}', file=sys.stderr)
        return 1

    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
