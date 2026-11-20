"""Plugin publication appends verified versions to the index and never rewrites history."""

import importlib.util
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


class Publication(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix='krabb plugins ')

        self.addCleanup(directory.cleanup)

        self.root = Path(directory.name)
        self.dist = self.root / 'dist'
        self.index = self.root / 'index.json'

        self.dist.mkdir()

        for plugin, version in [('echo', '0.2.0'), ('tools', '0.1.0')]:
            self.manifest(plugin, version)

    def manifest(self, plugin, version, group='channels'):
        path = self.root / 'krabb-plugins' / group / plugin / 'krabb-plugin.toml'

        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(MANIFEST.format(id=plugin, version=version))

    def archives(self, plugin, version):
        for target, extension in plugins.TARGETS.items():
            name = f'krabb-{plugin}-v{version}-{target}.{extension}'

            (self.dist / name).write_text(name)

    def publish(self, pullable=True):
        run = mock.Mock(return_value=mock.Mock(returncode=0, stderr=''))

        with mock.patch.object(plugins.subprocess, 'run', run), mock.patch('builtins.print'), \
                mock.patch.object(plugins, 'pullable', return_value=pullable):
            plugins.publish(self.index, self.dist, REGISTRY, SOURCE, root=self.root,
                            today='2026-11-20')

        return run

    def test_unpublished_versions_are_pushed_and_appended(self):
        self.index.write_text(json.dumps({
            'schema': 1,
            'registry': REGISTRY,
            'plugins': {'tools': [{'version': '0.1.0'}], 'echo': [{'version': '0.1.0'}]},
        }))

        found = plugins.manifests(self.root)

        self.assertEqual(plugins.unpublished(plugins.load(self.index), found), ['echo'])

        self.archives('echo', '0.2.0')

        run = self.publish()
        command = run.call_args.args[0]

        self.assertEqual(command[:3], ['oras', 'push', f'{REGISTRY}:echo-0.2.0'])
        self.assertIn(f'org.opencontainers.image.source={SOURCE}', command)
        self.assertEqual(run.call_args.kwargs['cwd'], self.dist)
        self.assertEqual(len([layer for layer in command if layer.startswith('krabb-echo')]), 6)

        value = json.loads(self.index.read_text())

        self.assertEqual(list(value['plugins']), ['echo', 'tools'])
        self.assertEqual(value['plugins']['echo'][0], {'version': '0.1.0'})

        release = value['plugins']['echo'][1]

        self.assertEqual(release['version'], '0.2.0')
        self.assertEqual(release['protocol'], {'major': 0, 'minor': 1})
        self.assertEqual(release['released'], '2026-11-20')
        self.assertEqual(release['permissions'], ['network'])
        self.assertEqual(release['secrets'], ['KRABB_ECHO_KEY'])

        linux = release['artifacts']['x86_64-unknown-linux-gnu']
        archive = self.dist / 'krabb-echo-v0.2.0-x86_64-unknown-linux-gnu.tar.gz'

        self.assertEqual(linux['sha256'], plugins.digest(archive))
        self.assertEqual(linux['size'], archive.stat().st_size)

        self.assertFalse(self.publish().called)

    def test_publication_stops_before_recording_unusable_versions(self):
        recorded = lambda: list(json.loads(self.index.read_text())['plugins'])

        self.archives('echo', '0.2.0')

        with self.assertRaisesRegex(plugins.Failure, 'tools 0.1.0 is missing its .* archive'):
            self.publish()

        self.assertEqual(recorded(), ['echo'])

        self.archives('tools', '0.1.0')

        with self.assertRaisesRegex(plugins.Failure, 'Make the package public'):
            self.publish(pullable=False)

        self.assertEqual(recorded(), ['echo'])

        oversized = self.dist / 'krabb-tools-v0.1.0-x86_64-unknown-linux-gnu.tar.gz'

        with mock.patch.object(plugins, 'ARCHIVE_LIMIT', oversized.stat().st_size - 1), \
                self.assertRaisesRegex(plugins.Failure, 'archive over 64 MiB'):
            self.publish()

        self.assertEqual(recorded(), ['echo'])

        failed = mock.Mock(return_value=mock.Mock(returncode=1, stderr='denied'))

        with mock.patch.object(plugins.subprocess, 'run', failed), \
                self.assertRaisesRegex(plugins.Failure, 'Pushing tools 0.1.0 failed: denied'):
            plugins.publish(self.index, self.dist, REGISTRY, SOURCE, root=self.root)

        self.assertEqual(recorded(), ['echo'])

    def test_indexes_and_manifests_are_validated(self):
        self.index.write_text(json.dumps({'schema': 1, 'registry': 'ghcr.io/x/y', 'plugins': {}}))

        with self.assertRaisesRegex(plugins.Failure, 'publishes to ghcr.io/x/y'):
            self.publish()

        self.index.write_text(json.dumps({'schema': 2, 'plugins': {}}))

        with self.assertRaisesRegex(plugins.Failure, 'schema 2'):
            plugins.load(self.index)

        with self.assertRaisesRegex(plugins.Failure, 'not a valid OCI repository'):
            plugins.publish(self.index, self.dist, 'GHCR.io', SOURCE, root=self.root)

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

    def test_command_line_reports_usage_and_failures(self):
        with mock.patch('sys.stderr'):
            self.assertEqual(plugins.main([]), 2)
            self.assertEqual(plugins.main(['plan', str(self.root / 'bad.json'), 'extra']), 2)

        self.index.write_text(json.dumps({'schema': 9, 'plugins': {}}))

        with mock.patch('sys.stderr'):
            self.assertEqual(plugins.main(['plan', str(self.index)]), 1)

        with mock.patch('builtins.print') as output:
            self.assertEqual(plugins.main(['plan', str(self.root / 'missing.json')]), 0)

        self.assertIn('telegram', output.call_args.args[0].split())


if __name__ == '__main__':
    unittest.main(verbosity=2)
