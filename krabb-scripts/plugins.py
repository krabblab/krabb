#!/usr/bin/env python3
"""Publish official plugin versions to their OCI registry and keep its index in sync.

The registry is the only record of what is published. Each version is one tag,
`<id>-<version>`, holding one archive per target and its manifest's declarations as
annotations. The `index` tag holds a summary rebuilt from those tags, so Krabb reads one
small document to choose a version.

plan REGISTRY
    Print the IDs of plugins whose manifest version has no tag in REGISTRY yet.

publish DIST REGISTRY SOURCE
    Push each of those versions from DIST, rebuild the index, and confirm that anonymous
    clients can pull it. Published versions are never changed.

official REGISTRY
    Print the IDs of the plugins with a version in REGISTRY, as a JSON array.

index REGISTRY
    Rebuild the index from the tags in REGISTRY, and push it when it changed.

withdraw REGISTRY ID VERSION REASON
    Mark a published version as withdrawn, then rebuild the index.

KRABB_REGISTRY_USER and KRABB_REGISTRY_TOKEN authenticate registry reads; pushes use the
credentials from `oras login`.
"""

import base64
from concurrent.futures import ThreadPoolExecutor
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib
import urllib.error
import urllib.parse
import urllib.request


ROOT = Path(__file__).resolve().parents[1]
SCHEMA = 1
ARTIFACT_TYPE = 'application/vnd.krabb.plugin.v1'
INDEX_TYPE = 'application/vnd.krabb.plugin-index.v1'
INDEX_MEDIA = 'application/vnd.krabb.plugin-index.v1+json'
INDEX_TAG = 'index'
OCI_MANIFEST = 'application/vnd.oci.image.manifest.v1+json'
ARCHIVE_LIMIT = 64 * 1024 * 1024
DOCUMENT_LIMIT = 4 * 1024 * 1024

TITLE = 'org.opencontainers.image.title'
VERSION_KEY = 'org.opencontainers.image.version'
CREATED = 'org.opencontainers.image.created'
SOURCE_KEY = 'org.opencontainers.image.source'
PROTOCOL = 'io.github.krabblab.plugin.protocol'
PERMISSIONS = 'io.github.krabblab.plugin.permissions'
SECRETS = 'io.github.krabblab.plugin.secrets'
WITHDRAWN = 'io.github.krabblab.plugin.withdrawn'

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
NEXT = re.compile(r'<([^>]+)>;\s*rel="next"')


class Failure(Exception):
    pass


class Redirects(urllib.request.HTTPRedirectHandler):
    """Stops at a blob redirect so the bearer token never reaches the storage host."""

    def redirect_request(self, *_):
        return None


class Registry:
    """Reads tags and manifests from one OCI repository."""

    def __init__(self, reference):
        if not REGISTRY.match(reference):
            raise Failure(f'Registry {reference} is not a valid OCI repository.')

        self.reference = reference
        self.host, self.repository = reference.split('/', 1)
        self.bearer = None

    def authorize(self):
        query = urllib.parse.urlencode(
            {'scope': f'repository:{self.repository}:pull', 'service': self.host},
        )

        request = urllib.request.Request(f'https://{self.host}/token?{query}')
        secret = os.environ.get('KRABB_REGISTRY_TOKEN')

        if secret:
            user = os.environ.get('KRABB_REGISTRY_USER') or 'token'
            credentials = base64.b64encode(f'{user}:{secret}'.encode()).decode()

            request.add_header('Authorization', f'Basic {credentials}')

        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                value = json.loads(response.read(64 * 1024))
        except (OSError, ValueError) as error:
            raise Failure(f'The registry {self.reference} refused a token: {error}.') from error

        token = value.get('token') or value.get('access_token')

        if not token:
            raise Failure(f'The registry {self.reference} returned no token.')

        return token

    def get(self, path, accept=None):
        """The JSON document at `path`, its headers, or None when it does not exist."""

        if self.bearer is None:
            self.bearer = self.authorize()

        try:
            return self.request(path, accept)
        except urllib.error.HTTPError as error:
            if error.code != 401:
                raise Failure(f'The registry {self.reference} answered {error.code} for {path}.')

        self.bearer = self.authorize()

        try:
            return self.request(path, accept)
        except urllib.error.HTTPError as error:
            raise Failure(f'The registry {self.reference} answered {error.code} for {path}.')

    def request(self, path, accept):
        """One request with the current token; a missing document is (None, None)."""

        headers = {'Authorization': f'Bearer {self.bearer}'}

        if accept:
            headers['Accept'] = accept

        url = path if path.startswith('https://') else f'https://{self.host}{path}'
        request = urllib.request.Request(url, headers=headers)
        opener = urllib.request.build_opener(Redirects)

        try:
            with opener.open(request, timeout=30) as response:
                return json.loads(response.read(DOCUMENT_LIMIT)), response.headers
        except urllib.error.HTTPError as error:
            if error.code == 404:
                return None, None

            raise
        except (OSError, ValueError) as error:
            raise Failure(f'The registry {self.reference} is unreachable: {error}.') from error

    def tags(self):
        found = []
        path = f'/v2/{self.repository}/tags/list?n=1000'

        while path:
            value, headers = self.get(path)

            if value is None:
                break

            found.extend(value.get('tags') or [])

            link = NEXT.search(headers.get('Link', '')) if headers else None
            path = link.group(1) if link else None

        return found

    def manifest(self, tag):
        return self.get(f'/v2/{self.repository}/manifests/{tag}', OCI_MANIFEST)[0]


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


def unpublished(registry, found):
    published = set(registry.tags())

    return [plugin for plugin, manifest in found.items()
            if f"{plugin}-{manifest['version']}" not in published]


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


def oras(command, cwd, failure):
    result = subprocess.run(['oras', *command], cwd=cwd, capture_output=True, text=True,
                            check=False)

    if result.returncode != 0:
        raise Failure(f'{failure}: {result.stderr.strip()}')


def push(registry, source, manifest, files, dist, created):
    plugin = manifest['id']
    version = manifest['version']
    protocol = manifest['protocol']
    layers = [f'{path.name}:{MEDIA[TARGETS[target]]}' for target, path in files.items()]

    annotations = {
        SOURCE_KEY: source,
        TITLE: plugin,
        VERSION_KEY: version,
        CREATED: created,
        PROTOCOL: f"{protocol['major']}.{protocol['minor']}",
        PERMISSIONS: json.dumps(manifest.get('permissions', [])),
        SECRETS: json.dumps(manifest.get('secrets', [])),
    }

    command = ['push', f'{registry}:{plugin}-{version}', '--artifact-type', ARTIFACT_TYPE]

    for key, value in annotations.items():
        command += ['--annotation', f'{key}={value}']

    oras([*command, *layers], dist, f'Pushing {plugin} {version} failed')


def release(tag, manifest):
    """One index entry from a version's manifest, or None when it is not a plugin version."""

    annotations = (manifest or {}).get('annotations') or {}
    plugin = annotations.get(TITLE)
    version = annotations.get(VERSION_KEY)
    protocol = annotations.get(PROTOCOL, '')

    if not (plugin and version and re.fullmatch(r'[0-9]+\.[0-9]+', protocol)) or \
            tag != f'{plugin}-{version}' or not ID.match(plugin) or not VERSION.match(version):
        return None

    artifacts = {}

    for layer in manifest.get('layers') or []:
        name = (layer.get('annotations') or {}).get(TITLE)

        digest = layer.get('digest')
        size = layer.get('size')

        if not isinstance(digest, str) or not isinstance(size, int):
            return None

        for target, extension in TARGETS.items():
            if name == f'krabb-{plugin}-v{version}-{target}.{extension}':
                artifacts[target] = {'sha256': digest.removeprefix('sha256:'), 'size': size}

    major, minor = protocol.split('.')

    try:
        permissions = json.loads(annotations.get(PERMISSIONS, '[]'))
        secrets = json.loads(annotations.get(SECRETS, '[]'))
    except ValueError:
        return None

    if not all(isinstance(value, list) and all(isinstance(item, str) for item in value)
               for value in [permissions, secrets]):
        return None

    entry = {
        'version': version,
        'protocol': {'major': int(major), 'minor': int(minor)},
        'released': annotations.get(CREATED, '')[:10],
        'permissions': permissions,
        'secrets': secrets,
        'artifacts': artifacts,
    }

    if annotations.get(WITHDRAWN):
        entry['yanked'] = annotations[WITHDRAWN]

    return plugin, entry


def document(registry):
    """The index built from every version tag, with its digest, version count, and the tags
    it leaves out."""

    tags = sorted(tag for tag in registry.tags() if tag != INDEX_TAG)

    with ThreadPoolExecutor(max_workers=8) as pool:
        found = list(pool.map(registry.manifest, tags))

    plugins = {}
    foreign = []

    for tag, manifest in zip(tags, found):
        entry = release(tag, manifest)

        if entry is None:
            foreign.append(tag)
            continue

        plugins.setdefault(entry[0], []).append(entry[1])

    value = {'schema': SCHEMA, 'registry': registry.reference, 'plugins': plugins}
    content = (json.dumps(value, indent=2) + '\n').encode()
    count = sum(len(entries) for entries in plugins.values())

    return content, hashlib.sha256(content).hexdigest(), count, foreign


def official(registry):
    """The IDs of the plugins the index lists, sorted."""

    content = document(registry)[0]

    return sorted(json.loads(content)['plugins'])


def versions(count):
    return f"{count} plugin version{'' if count == 1 else 's'}"


def leftover(tags):
    for tag in tags:
        print(f'[WARN] Tag {tag} has no Krabb plugin metadata, so the index leaves it out.',
              file=sys.stderr)


def index(registry, attempts=5):
    """Rebuilds the index from every version tag and pushes it when it changed.

    A withdrawal may change the registry meanwhile, so after each push the index is rebuilt
    again until it matches what the registry holds. Returns the digest of the index
    document."""

    for attempt in range(attempts + 1):
        content, digest, count, foreign = document(registry)
        current = registry.manifest(INDEX_TAG)
        layers = (current or {}).get('layers') or [{}]

        if layers[0].get('digest') == f'sha256:{digest}':
            leftover(foreign)

            if attempt:
                print(f'[INFO] Published the plugin index with {versions(count)}.')
            else:
                print(f'[INFO] The plugin index is current with {versions(count)}.')

            return digest

        if attempt == attempts:
            leftover(foreign)
            break

        with tempfile.TemporaryDirectory(prefix='krabb-index-') as directory:
            Path(directory, 'index.json').write_bytes(content)

            oras(['push', f'{registry.reference}:{INDEX_TAG}', '--artifact-type', INDEX_TYPE,
                  f'index.json:{INDEX_MEDIA}'], directory, 'Pushing the plugin index failed')

    raise Failure('The plugin index kept changing while it was rebuilt; run the workflow '
                  'again.')


def publish(dist, reference, source, root=ROOT, now=None):
    registry = Registry(reference)
    found = manifests(root)
    created = (now or datetime.datetime.now(datetime.timezone.utc)).strftime('%Y-%m-%dT%H:%M:%SZ')
    digests = []

    for plugin in unpublished(registry, found):
        manifest = found[plugin]
        files = archives(dist, plugin, manifest['version'])

        push(reference, source, manifest, files, dist, created)
        digests.extend(digest(path) for path in files.values())
        print(f"[INFO] Published {plugin} {manifest['version']}.")

    digests.append(index(registry))

    if not pullable(reference, digests):
        raise Failure(
            f'Anonymous clients cannot pull from {reference}. Make the package public in its '
            'GitHub settings, then run the Release workflow again.'
        )


def withdraw(reference, plugin, version, reason):
    if not ID.match(plugin) or not VERSION.match(version):
        raise Failure(f'Plugin {plugin} {version} is not a valid plugin version.')

    if not reason.strip():
        raise Failure('Give a reason for withdrawing the version.')

    registry = Registry(reference)
    tag = f'{plugin}-{version}'
    manifest = registry.manifest(tag)

    if release(tag, manifest) is None:
        raise Failure(f'Plugin {plugin} {version} is not published in {reference}.')

    manifest.setdefault('annotations', {})[WITHDRAWN] = reason.strip()

    with tempfile.TemporaryDirectory(prefix='krabb-withdraw-') as directory:
        Path(directory, 'manifest.json').write_text(json.dumps(manifest))

        oras(['manifest', 'push', '--media-type', OCI_MANIFEST, f'{reference}:{tag}',
              'manifest.json'], directory, f'Withdrawing {plugin} {version} failed')

    print(f'[INFO] Withdrew {plugin} {version}.')
    index(registry)


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


def main(arguments):
    try:
        match arguments:
            case ['plan', registry]:
                print(' '.join(unpublished(Registry(registry), manifests())))
            case ['publish', dist, registry, source]:
                publish(Path(dist), registry, source)
            case ['official', registry]:
                print(json.dumps(official(Registry(registry)), separators=(',', ':')))
            case ['index', registry]:
                index(Registry(registry))
            case ['withdraw', registry, plugin, version, reason]:
                withdraw(registry, plugin, version, reason)
            case _:
                print(__doc__.strip(), file=sys.stderr)
                return 2
    except Failure as error:
        print(f'[ERROR] {error}', file=sys.stderr)
        return 1

    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
