"""Plugin publication pushes new versions and rebuilds the index from the registry alone."""

import importlib.util
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('plugins', ROOT / 'krabb-scripts/plugins.py')
plugins = importlib.util.module_from_spec(SPEC)

SPEC.loader.exec_module(plugins)

REGISTRY = 'ghcr.io/krabblab/krabb/plugins'
SOURCE = 'https://github.com/krabblab/krabb'

MANIFEST = '''id = "{id}"
version = "{version}"
capabilities = ["tool"]
permissions = ["network"]
secrets = ["KRABB_ECHO_KEY"]

[protocol]
major = 0
minor = 1
'''


def layer(name, digest='a' * 64, size=10):
    return {'digest': f'sha256:{digest}', 'size': size,
            'annotations': {plugins.TITLE: name}}


def version_manifest(plugin, version, withdrawn=None, protocol='0.1'):
    annotations = {
        plugins.TITLE: plugin,
        plugins.VERSION_KEY: version,
        plugins.CREATED: '2026-11-20T10:00:00Z',
        plugins.PROTOCOL: protocol,
        plugins.PERMISSIONS: '["network"]',
        plugins.SECRETS: '[]',
    }

    if withdrawn:
        annotations[plugins.WITHDRAWN] = withdrawn

    layers = [layer(f'krabb-{plugin}-v{version}-{target}.{extension}')
              for target, extension in plugins.TARGETS.items()]

    return {'annotations': annotations, 'layers': layers}


class Fake(plugins.Registry):
    """An in-memory registry that records what `oras` pushes to it."""

    def __init__(self, manifests=None):
        super().__init__(REGISTRY)

        self.manifests = dict(manifests or {})
        self.blobs = {}

    def tags(self):
        return list(self.manifests)

    def manifest(self, tag):
        return self.manifests.get(tag)

    def blob(self, digest):
        return self.blobs.get(digest)

    def oras(self, command, cwd, failure):
        if command[0] == 'manifest':
            tag = command[-2].split(':')[-1]
            self.manifests[tag] = json.loads(Path(cwd, command[-1]).read_text())

            return

        tag = command[1].split(':')[-1]
        annotations = {}
        layers = []
        rest = iter(command[4:])

        for item in rest:
            if item == '--annotation':
                key, value = next(rest).split('=', 1)
                annotations[key] = value
            else:
                name = item.split(':')[0]
                content = Path(cwd, name).read_bytes()

                layers.append(layer(name, hashlib.sha256(content).hexdigest(), len(content)))

        self.manifests[tag] = {'annotations': annotations, 'layers': layers}


class Publication(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix='krabb plugins ')

        self.addCleanup(directory.cleanup)

        self.root = Path(directory.name)
        self.dist = self.root / 'dist'

        self.dist.mkdir()

        for plugin, version in [('echo', '0.2.0'), ('tools', '0.1.0')]:
            self.manifest(plugin, version)

    def manifest(self, plugin, version, group='channels', crate=None):
        path = self.root / 'krabb-plugins' / group / plugin / 'krabb-plugin.toml'

        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(MANIFEST.format(id=plugin, version=version))
        (path.parent / 'Cargo.toml').write_text(f'[package]\nversion = "{crate or version}"\n')

    def archives(self, plugin, version):
        for target, extension in plugins.TARGETS.items():
            name = f'krabb-{plugin}-v{version}-{target}.{extension}'

            (self.dist / name).write_text(name)

    def publish(self, registry, pullable=True):
        with mock.patch.object(plugins, 'Registry', return_value=registry), \
                mock.patch.object(plugins, 'oras', registry.oras), \
                mock.patch.object(plugins, 'pullable', return_value=pullable) as checked, \
                mock.patch('builtins.print'):
            plugins.publish(self.dist, REGISTRY, SOURCE, root=self.root,
                            now=plugins.datetime.datetime(2026, 11, 21, 9, 30))

        return checked

    def test_unpublished_versions_are_pushed_and_indexed(self):
        registry = Fake({'tools-0.1.0': version_manifest('tools', '0.1.0'),
                         'echo-0.1.0': version_manifest('echo', '0.1.0', 'crashes on start')})

        self.assertEqual(plugins.unpublished(registry, plugins.manifests(self.root)), ['echo'])

        self.archives('echo', '0.2.0')

        pushed = mock.Mock(wraps=registry.oras)
        registry.oras = pushed
        checked = self.publish(registry)
        command = pushed.call_args_list[0].args[0]

        self.assertEqual(command[:2], ['push', f'{REGISTRY}:echo-0.2.0'])
        self.assertIn(f'{plugins.SOURCE_KEY}={SOURCE}', command)
        self.assertIn(f'{plugins.PROTOCOL}=0.1', command)
        self.assertIn(f'{plugins.CREATED}=2026-11-21T09:30:00Z', command)
        self.assertEqual(pushed.call_args_list[0].args[1], self.dist)
        self.assertEqual(len([item for item in command if item.startswith('krabb-echo')]), 6)
        self.assertEqual(pushed.call_args_list[1].args[0][1], f'{REGISTRY}:index')
        self.assertEqual(len(checked.call_args.args[1]), 7)

        entries = {}

        with mock.patch('builtins.print'):
            digest = plugins.index(registry)

        self.assertEqual(registry.manifests['index']['layers'][0]['digest'], f'sha256:{digest}')

        for tag in ['echo-0.1.0', 'echo-0.2.0', 'tools-0.1.0']:
            plugin, entry = plugins.release(tag, registry.manifests[tag])

            entries.setdefault(plugin, []).append(entry)

        release = entries['echo'][1]

        self.assertEqual(entries['echo'][0]['yanked'], 'crashes on start')
        self.assertEqual(release['version'], '0.2.0')
        self.assertEqual(release['protocol'], {'major': 0, 'minor': 1})
        self.assertEqual(release['released'], '2026-11-21')
        self.assertEqual(release['permissions'], ['network'])
        self.assertEqual(release['secrets'], ['KRABB_ECHO_KEY'])
        self.assertNotIn('yanked', release)

        linux = release['artifacts']['x86_64-unknown-linux-gnu']
        archive = self.dist / 'krabb-echo-v0.2.0-x86_64-unknown-linux-gnu.tar.gz'

        self.assertEqual(linux['sha256'], plugins.digest(archive))
        self.assertEqual(linux['size'], archive.stat().st_size)

        pushed.reset_mock()
        self.publish(registry)

        self.assertFalse(pushed.called)

    def test_the_index_is_rebuilt_from_tags_and_skips_foreign_ones(self):
        registry = Fake({'tools-0.1.0': version_manifest('tools', '0.1.0'),
                         'a2a-0.1.0': {'annotations': {plugins.TITLE: 'a2a'}, 'layers': []},
                         'echo-9.9.9': version_manifest('echo', '0.1.0')})

        with mock.patch.object(plugins, 'oras', registry.oras), \
                mock.patch('builtins.print') as output:
            digest = plugins.index(registry)

        warnings = [call.args[0] for call in output.call_args_list
                    if call.kwargs.get('file') is plugins.sys.stderr]

        self.assertEqual(len(warnings), 2)
        self.assertIn('Tag a2a-0.1.0 has no Krabb plugin metadata', warnings[0])

        with mock.patch.object(plugins, 'oras', side_effect=AssertionError), \
                mock.patch('builtins.print') as output:
            self.assertEqual(plugins.index(registry), digest)

        output.assert_any_call('[INFO] The plugin index is current with 1 plugin version.')

    def test_the_index_is_rebuilt_until_it_matches_concurrent_changes(self):
        registry = Fake({'tools-0.1.0': version_manifest('tools', '0.1.0')})
        pushes = []

        def oras(command, cwd, failure):
            registry.oras(command, cwd, failure)
            pushes.append(command[1])

            if len(pushes) == 1:
                registry.manifests['echo-0.1.0'] = version_manifest('echo', '0.1.0')

        with mock.patch.object(plugins, 'oras', oras), mock.patch('builtins.print'):
            digest = plugins.index(registry)

        self.assertEqual(len(pushes), 2)
        self.assertEqual(plugins.document(registry)[1], digest)

        changes = []

        def unstable(command, cwd, failure):
            registry.oras(command, cwd, failure)
            changes.append(command[1])

            registry.manifests[f'echo-1.{len(changes)}.0'] = version_manifest(
                'echo', f'1.{len(changes)}.0')

        registry.manifests['echo-0.9.0'] = version_manifest('echo', '0.9.0')

        with mock.patch.object(plugins, 'oras', unstable), mock.patch('builtins.print'), \
                self.assertRaisesRegex(plugins.Failure, 'kept changing'):
            plugins.index(registry)

        self.assertEqual(len(changes), 5)

    def test_publication_stops_on_unusable_versions(self):
        registry = Fake()

        self.archives('echo', '0.2.0')

        with self.assertRaisesRegex(plugins.Failure, 'tools 0.1.0 is missing its .* archive'):
            self.publish(registry)

        self.archives('tools', '0.1.0')

        oversized = self.dist / 'krabb-tools-v0.1.0-x86_64-unknown-linux-gnu.tar.gz'

        with mock.patch.object(plugins, 'ARCHIVE_LIMIT', oversized.stat().st_size - 1), \
                self.assertRaisesRegex(plugins.Failure, 'archive over 64 MiB'):
            self.publish(registry)

        with self.assertRaisesRegex(plugins.Failure, 'Make the package public'):
            self.publish(registry, pullable=False)

        self.assertIn('index', registry.manifests)

        failed = mock.Mock(return_value=mock.Mock(returncode=1, stderr='denied'))

        with mock.patch.object(plugins.subprocess, 'run', failed), \
                self.assertRaisesRegex(plugins.Failure, 'Pushing tools 0.1.0 failed: denied'):
            plugins.push(REGISTRY, SOURCE, plugins.manifests(self.root)['tools'],
                         plugins.archives(self.dist, 'tools', '0.1.0'), self.dist, 'now')

    def test_withdrawal_annotates_the_version_and_rebuilds_the_index(self):
        registry = Fake({'echo-0.1.0': version_manifest('echo', '0.1.0')})

        def withdraw(*arguments):
            with mock.patch.object(plugins, 'Registry', return_value=registry), \
                    mock.patch.object(plugins, 'oras', registry.oras), \
                    mock.patch('builtins.print'):
                plugins.withdraw(REGISTRY, *arguments)

        withdraw('echo', '0.1.0', ' Crashes on start. ')

        annotations = registry.manifests['echo-0.1.0']['annotations']

        self.assertEqual(annotations[plugins.WITHDRAWN], 'Crashes on start.')
        self.assertIn('index', registry.manifests)

        with self.assertRaisesRegex(plugins.Failure, 'echo 0.2.0 is not published'):
            withdraw('echo', '0.2.0', 'Broken.')

        with self.assertRaisesRegex(plugins.Failure, 'Give a reason'):
            withdraw('echo', '0.1.0', ' ')

    def test_registry_reads_authenticate_page_and_treat_missing_as_empty(self):
        page = '</v2/krabblab/krabb/plugins/tags/list?n=1&last=e>; rel="next"'
        responses = [({'tags': ['echo-0.1.0']}, {'Link': page}), ({'tags': ['tools-0.1.0']}, {})]

        registry = plugins.Registry(REGISTRY)
        registry.bearer = 'token'

        with mock.patch.object(registry, 'get', side_effect=responses) as get:
            self.assertEqual(registry.tags(), ['echo-0.1.0', 'tools-0.1.0'])

        self.assertTrue(get.call_args_list[1].args[0].endswith('last=e'))

        with mock.patch.object(registry, 'get', return_value=(None, None)):
            self.assertEqual(registry.tags(), [])

        token = mock.MagicMock()
        token.__enter__.return_value.read.return_value = b'{"token": "pull"}'

        with mock.patch.dict(plugins.os.environ, {'KRABB_REGISTRY_TOKEN': 'secret',
                                                  'KRABB_REGISTRY_USER': 'bot'}), \
                mock.patch.object(plugins.urllib.request, 'urlopen', return_value=token) as open:
            self.assertEqual(plugins.Registry(REGISTRY).authorize(), 'pull')

        self.assertTrue(open.call_args.args[0].get_header('Authorization').startswith('Basic '))

        document = mock.MagicMock()
        document.__enter__.return_value.read.return_value = b'{"layers": []}'
        missing = plugins.urllib.error.HTTPError('url', 404, 'missing', {}, None)
        denied = plugins.urllib.error.HTTPError('url', 401, 'denied', {}, None)
        opener = mock.Mock(open=mock.Mock(side_effect=[document, missing, denied, denied]))

        with mock.patch.object(plugins.urllib.request, 'build_opener', return_value=opener), \
                mock.patch.object(registry, 'authorize', return_value='token'):
            self.assertEqual(registry.manifest('index'), {'layers': []})
            self.assertIsNone(registry.manifest('echo-0.2.0'))

            with self.assertRaisesRegex(plugins.Failure, 'answered 401'):
                registry.manifest('echo-0.3.0')

        request = opener.open.call_args_list[0].args[0]

        self.assertEqual(request.get_header('Authorization'), 'Bearer token')
        self.assertEqual(request.get_header('Accept'), plugins.OCI_MANIFEST)

        expired = plugins.urllib.error.HTTPError('url', 401, 'expired', {}, None)
        opener = mock.Mock(open=mock.Mock(side_effect=[expired, document]))

        with mock.patch.object(plugins.urllib.request, 'build_opener', return_value=opener), \
                mock.patch.object(registry, 'authorize', return_value='fresh') as authorize:
            self.assertEqual(registry.manifest('index'), {'layers': []})

        authorize.assert_called_once()
        self.assertEqual(opener.open.call_args.args[0].get_header('Authorization'), 'Bearer fresh')

    def test_malformed_versions_are_left_out_of_the_index(self):
        valid = version_manifest('echo', '0.1.0')

        self.assertIsNotNone(plugins.release('echo-0.1.0', valid))

        for change in [
            lambda manifest: manifest['annotations'].update({plugins.PERMISSIONS: '{bad'}),
            lambda manifest: manifest['annotations'].update({plugins.SECRETS: '[1]'}),
            lambda manifest: manifest['layers'][0].pop('size'),
            lambda manifest: manifest['annotations'].update({plugins.PROTOCOL: 'one'}),
        ]:
            manifest = version_manifest('echo', '0.1.0')

            change(manifest)

            self.assertIsNone(plugins.release('echo-0.1.0', manifest))

        self.assertIsNone(plugins.release('echo-0.1.0', None))

    def test_registries_and_manifests_are_validated(self):
        with self.assertRaisesRegex(plugins.Failure, 'not a valid OCI repository'):
            plugins.publish(self.dist, 'GHCR.io', SOURCE, root=self.root)

        self.manifest('echo', '0.2.0', group='clients')

        with self.assertRaisesRegex(plugins.Failure, 'more than one manifest'):
            plugins.manifests(self.root)

        self.manifest('My_Plugin', '0.1.0', group='clients')

        with self.assertRaisesRegex(plugins.Failure, 'Plugin ID My_Plugin is not valid'):
            plugins.manifests(self.root)

        (self.root / 'krabb-plugins/clients/My_Plugin/krabb-plugin.toml').unlink()
        self.manifest('echo', 'main', group='channels')
        (self.root / 'krabb-plugins/clients/echo/krabb-plugin.toml').unlink()

        with self.assertRaisesRegex(plugins.Failure, 'invalid version: main'):
            plugins.manifests(self.root)

        self.manifest('echo', '0.2.0-rc.1', crate='0.1.0')

        with self.assertRaisesRegex(plugins.Failure, '0.2.0-rc.1 in its manifest but 0.1.0'):
            plugins.manifests(self.root)

        cargo = self.root / 'krabb-plugins/channels/echo/Cargo.toml'

        for content in ['[package]\nversion.workspace = true\n', 'package = 1\n']:
            cargo.write_text(content)

            with self.assertRaisesRegex(plugins.Failure, 'needs its own version in Cargo.toml'):
                plugins.manifests(self.root)

    def test_pull_checks_stop_at_the_storage_redirect(self):
        token = mock.MagicMock()
        token.__enter__.return_value.read.return_value = b'{"token": "anonymous"}'

        def check(code):
            error = plugins.urllib.error.HTTPError('url', code, 'status', {}, None)
            opener = mock.Mock(open=mock.Mock(side_effect=error))

            with mock.patch.object(plugins.urllib.request, 'urlopen', return_value=token), \
                    mock.patch.object(plugins.urllib.request, 'build_opener',
                                      return_value=opener):
                result = plugins.pullable(REGISTRY, ['a' * 64, 'b' * 64])

            request = opener.open.call_args.args[0]

            self.assertEqual(request.get_method(), 'HEAD')
            self.assertEqual(request.get_header('Authorization'), 'Bearer anonymous')

            return result

        self.assertTrue(check(307))
        self.assertFalse(check(401))

    def test_repository_manifests_are_publishable(self):
        found = plugins.manifests()

        self.assertIn('telegram', found)
        self.assertTrue(all(plugins.VERSION.match(item['version']) for item in found.values()))

    def test_repository_manifests_are_publishable(self):
        found = plugins.manifests()

        self.assertIn('telegram', found)
        self.assertTrue(all(plugins.VERSION.match(item['version']) for item in found.values()))

    def test_command_line_reports_usage_and_failures(self):
        with mock.patch('sys.stderr'):
            self.assertEqual(plugins.main([]), 2)
            self.assertEqual(plugins.main(['plan', REGISTRY, 'extra']), 2)
            self.assertEqual(plugins.main(['plan', 'GHCR.io']), 1)

        registry = Fake({'telegram-0.0.1': version_manifest('telegram', '0.0.1')})

        with mock.patch.object(plugins, 'Registry', return_value=registry), \
                mock.patch('builtins.print') as output:
            self.assertEqual(plugins.main(['plan', REGISTRY]), 0)

        self.assertIn('telegram', output.call_args.args[0].split())

    def test_ids_are_read_from_the_published_index(self):
        registry = Fake()
        digest = 'sha256:' + 'c' * 64
        registry.manifests['index'] = {'layers': [{'digest': digest}]}
        registry.blobs = {digest: {'plugins': {'tools': [], 'echo': []}}}

        with mock.patch.object(plugins, 'Registry', return_value=registry), \
                mock.patch('builtins.print') as output:
            self.assertEqual(plugins.main(['ids', REGISTRY]), 0)

        self.assertEqual(output.call_args.args[0], '["echo","tools"]')

        for index in [None, {'layers': []}, {'layers': [{'digest': '../../tags'}]}]:
            registry.manifests['index'] = index

            with self.assertRaisesRegex(plugins.Failure, 'no readable plugin index'):
                plugins.ids(registry)

    def test_redirected_blobs_are_fetched_without_the_token(self):
        registry = plugins.Registry(REGISTRY)
        registry.bearer = 'secret'
        storage = 'https://pkg-containers.githubusercontent.com/blob?signed'

        redirect = plugins.urllib.error.HTTPError('url', 307, 'redirect',
                                                  {'Location': storage}, None)

        opener = mock.Mock(open=mock.Mock(side_effect=redirect))

        stored = mock.MagicMock()
        stored.__enter__.return_value.read.return_value = b'{"plugins": {}}'

        with mock.patch.object(plugins.urllib.request, 'build_opener', return_value=opener), \
                mock.patch.object(plugins.urllib.request, 'urlopen',
                                  return_value=stored) as fetched:
            self.assertEqual(registry.blob('sha256:' + 'd' * 64), {'plugins': {}})

        self.assertEqual(fetched.call_args.args[0], storage)


if __name__ == '__main__':
    unittest.main(verbosity=2)
