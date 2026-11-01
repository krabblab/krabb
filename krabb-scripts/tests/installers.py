"""Local release artifacts must pass integrity checks before activation or delegation."""

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
                        KRABB_FIXTURE_CALLS=str(self.calls))

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

    def run_installer(self, plugin):
        executable = shutil.which('pwsh' if WINDOWS else 'sh')

        if not executable:
            self.skipTest('installer interpreter is unavailable')

        args = [executable, '-NoProfile', '-File'] if WINDOWS else [executable]

        return subprocess.run([*args, str(INSTALLER), plugin], env=self.env,
                              capture_output=True, text=True, timeout=20)

    def test_rejects_downloads_before_replacement_or_plugin_delegation(self):
        for plugin in ['core', 'tools']:
            for checksum in ['mismatch', 'missing']:
                with self.subTest(plugin=plugin, checksum=checksum):
                    self.artifact(plugin, checksum)

                    result = self.run_installer(plugin)

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

    def test_valid_checksum_delegates_verified_plugin(self):
        archive, digest, _ = self.artifact('tools', 'valid')
        result = self.run_installer('tools')

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

        call = self.calls.read_text()

        for argument in ['plugin', 'install', 'tools',
                         archive.as_uri() + '#sha256=' + digest, '--yes']:
            self.assertIn(argument, call)

        for name, content in self.installed.items():
            self.assertEqual((self.bin / name).read_bytes(), content)


if __name__ == '__main__':
    unittest.main(verbosity=2)
