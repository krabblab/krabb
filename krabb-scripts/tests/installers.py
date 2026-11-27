"""Host release archives must pass integrity checks; plugins are delegated to the index."""

import hashlib
import io
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile
import unittest
import zipfile


ROOT = Path(__file__).resolve().parents[2]
WINDOWS = os.name == 'nt'
INSTALLER = ROOT / ('install.ps1' if WINDOWS else 'install.sh')


class Installers(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='krabb installer ')

        self.addCleanup(self.directory.cleanup)

        self.root = Path(self.directory.name)
        self.release = self.root / 'release'

        self.release.mkdir()

        self.bin = self.root / ('local/Krabb/bin' if WINDOWS else 'prefix/bin')

        self.bin.mkdir(parents=True)

        self.extension = '.exe' if WINDOWS else ''

        self.installed = {name + self.extension: ('previous ' + name).encode()
                          for name in ['krabb', 'krabb-daemon']}

        for name, content in self.installed.items():
            (self.bin / name).write_bytes(content)

        self.calls = self.root / 'delegated'
        delegate = self.root / ('delegate.cmd' if WINDOWS else 'delegate')

        if WINDOWS:
            delegate.write_text('@echo off\r\necho %* > "%KRABB_FIXTURE_CALLS%"\r\nexit /b 0\r\n')
        else:
            delegate.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$KRABB_FIXTURE_CALLS"\n')
            delegate.chmod(0o755)

        self.env = dict(os.environ)

        self.env.update(KRABB_VERSION='1.2.3', KRABB_RELEASE_BASE=self.release.as_uri(),
                        KRABB_PREFIX=str(self.root / 'prefix'),
                        LOCALAPPDATA=str(self.root / 'local'),
                        KRABB_HOME=str(self.root / 'config'), KRABB_BIN=str(delegate),
                        KRABB_FIXTURE_CALLS=str(self.calls), KRABB_KEEP_PATH='1')

    def artifact(self, plugin, checksum):
        arch = 'aarch64' if platform.machine().lower() in ('arm64', 'aarch64') else 'x86_64'

        target = ('pc-windows-msvc' if WINDOWS else
                  'apple-darwin' if platform.system() == 'Darwin' else 'unknown-linux-gnu')

        suffix = 'zip' if WINDOWS else 'tar.gz'
        archive = self.release / f'krabb-{plugin}-v1.2.3-{arch}-{target}.{suffix}'

        payload = {name + self.extension: ('replacement ' + name).encode()
                   for name in ['krabb', 'krabb-daemon']}

        if WINDOWS:
            with zipfile.ZipFile(archive, 'w') as output:
                for name, content in payload.items():
                    output.writestr('package/' + name, content)
        else:
            with tarfile.open(archive, 'w:gz') as output:
                for name, content in payload.items():
                    entry = tarfile.TarInfo('package/' + name)
                    entry.size = len(content)
                    entry.mode = 0o755

                    output.addfile(entry, io.BytesIO(content))

        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        expected = '0' * 64 if checksum == 'mismatch' else digest
        name = 'unrelated.tar.gz' if checksum == 'missing' else archive.name

        (self.release / 'SHA256SUMS').write_text(f'{expected}  {name}\n')
        return archive, digest, payload

    def run_installer(self, plugin, **env):
        executable = shutil.which('pwsh' if WINDOWS else 'sh')

        if not executable:
            self.skipTest('installer interpreter is unavailable')

        args = [executable, '-NoProfile', '-File'] if WINDOWS else [executable]

        return subprocess.run([*args, str(INSTALLER), plugin], env={**self.env, **env},
                              capture_output=True, text=True, timeout=20)

    def test_rejects_downloads_before_replacement(self):
        for checksum in ['mismatch', 'missing']:
            with self.subTest(checksum=checksum):
                self.artifact('core', checksum)

                result = self.run_installer('core')

                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)

                message = ('Checksum is missing' if checksum == 'missing' else
                           'Checksum verification failed')

                self.assertIn(message, result.stderr)

                for name, content in self.installed.items():
                    self.assertEqual((self.bin / name).read_bytes(), content)

                self.assertFalse(self.calls.exists(), 'rejected download was delegated')

    def test_valid_checksum_replaces_both_binaries(self):
        _, _, payload = self.artifact('core', 'valid')
        result = self.run_installer('core')

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

        for name, content in payload.items():
            self.assertEqual((self.bin / name).read_bytes(), content)

        self.assertFalse(self.calls.exists())

    def test_unpinned_installs_take_the_latest_release_version(self):
        archive, _, payload = self.artifact('core', 'valid')
        result = self.run_installer('core', KRABB_VERSION='')

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f'Downloading {archive.name}.', result.stdout)

        for name, content in payload.items():
            self.assertEqual((self.bin / name).read_bytes(), content)

        (self.release / 'SHA256SUMS').write_text(f'{"0" * 64}  krabb-core-v1.2.3-plan9.zip\n')

        result = self.run_installer('core', KRABB_VERSION='')

        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('The latest Krabb release has no build for', result.stderr)

        (self.release / 'SHA256SUMS').unlink()

        result = self.run_installer('core', KRABB_VERSION='')

        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('Cannot download SHA256SUMS from file:', result.stderr)

    def test_release_tags_and_bare_versions_both_install(self):
        _, _, payload = self.artifact('core', 'valid')

        result = self.run_installer('core', KRABB_VERSION='v1.2.3')

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

        for name, content in payload.items():
            self.assertEqual((self.bin / name).read_bytes(), content)

        result = self.run_installer('core', KRABB_VERSION='latest')

        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('Use a version such as 0.1.0 or 0.1.0-rc.1.', result.stderr)

    def test_installs_end_with_the_path_and_the_first_command(self):
        self.artifact('core', 'valid')

        result = self.run_installer('core')

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('\n\n[INFO] Set up Krabb with `krabb init`.', result.stdout)
        self.assertNotIn('\n\n\n', result.stdout)

        if not WINDOWS:
            self.assertIn(f'export PATH="{self.bin}:$PATH"', result.stdout)
            self.assertIn('.\n\n[INFO] Add ', result.stdout)

            path = f"{self.bin}:{os.environ['PATH']}"
            result = self.run_installer('core', PATH=path)

            self.assertNotIn('export PATH=', result.stdout)

    def test_unpinned_installs_name_the_newest_prerelease_before_a_stable_one(self):
        releases = self.root / 'releases.json'

        releases.write_text('[\n  {\n    "tag_name": "v0.1.0-rc.1",\n'
                            '    "prerelease": true\n  }\n]\n')

        result = self.run_installer('core', KRABB_VERSION='', KRABB_RELEASE_LIST=releases.as_uri())

        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('No stable Krabb release is published yet', result.stderr)
        self.assertIn('The newest pre-release is 0.1.0-rc.1;', result.stderr)
        self.assertIn('KRABB_VERSION', result.stderr)

        for release in ['"v0.1.0", "prerelease": false', '"v0.1.0;rm -rf ~", "prerelease": true']:
            with self.subTest(release=release):
                releases.write_text(f'[{{"tag_name": {release}}}]\n')

                result = self.run_installer('core', KRABB_VERSION='',
                                            KRABB_RELEASE_LIST=releases.as_uri())

                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertNotIn('pre-release', result.stderr)
                self.assertIn('Cannot download SHA256SUMS', result.stderr)

    def test_plugins_are_delegated_to_the_plugin_index(self):
        result = self.run_installer('tools', KRABB_VERSION='', KRABB_RELEASE_BASE='')

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

        call = self.calls.read_text().split()

        self.assertEqual(call, ['plugin', 'install', 'tools', '--yes'])

        for name, content in self.installed.items():
            self.assertEqual((self.bin / name).read_bytes(), content)

    def test_plugin_versions_are_passed_to_the_plugin_index(self):
        result = self.run_installer('tools@0.1.0', KRABB_VERSION='', KRABB_RELEASE_BASE='')

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

        self.assertEqual(self.calls.read_text().split(),
                         ['plugin', 'install', 'tools@0.1.0', '--yes'])

    def test_core_versions_point_to_the_version_variable(self):
        result = self.run_installer('core@1.2.3')

        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('Set KRABB_VERSION', result.stderr)
        self.assertFalse(self.calls.exists())

    def test_invalid_plugin_ids_are_rejected(self):
        for plugin in ['../tools', 'tools@', 'tools@1@2', 'Tools', 'tools\nok']:
            with self.subTest(plugin=plugin):
                result = self.run_installer(plugin, KRABB_VERSION='')

                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn('is not valid', result.stderr)
                self.assertFalse(self.calls.exists())

    def test_plugin_installs_refuse_core_variables(self):
        for env, message in [({}, 'install a plugin version with tools@VERSION'),
                             ({'KRABB_VERSION': ''}, 'plugins come from the plugin index')]:
            with self.subTest(message=message):
                result = self.run_installer('tools', **env)

                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn(message, result.stderr)
                self.assertFalse(self.calls.exists())


if __name__ == '__main__':
    unittest.main(verbosity=2)
