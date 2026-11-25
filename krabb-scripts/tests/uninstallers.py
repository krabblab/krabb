"""The uninstall scripts remove Krabb software by default and everything only on --purge."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
WINDOWS = os.name == 'nt'
UNINSTALLER = ROOT / ('uninstall.ps1' if WINDOWS else 'uninstall.sh')


class Uninstallers(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='krabb uninstaller ')

        self.addCleanup(self.directory.cleanup)

        self.root = Path(self.directory.name)
        extension = '.exe' if WINDOWS else ''

        # Every location points inside the fixture, so no real installation or service is touched.
        self.bin = self.root / ('local/Krabb/bin' if WINDOWS else 'prefix/bin')
        self.home = self.root / 'krabb-home'
        self.binaries = [self.bin / ('krabb' + extension), self.bin / ('krabb-daemon' + extension)]

        self.bin.mkdir(parents=True)

        for binary in self.binaries:
            binary.write_text('binary')

        for name in ['plugins/echo/bin/krabb-plugin-echo', 'cache/index.json',
                     'updates/v0.2.0/krabb.tar.gz', 'updates/recovery/point.json',
                     'config.toml', 'credentials.json', 'data/sessions/index.json']:
            path = self.home / name

            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name)

        self.env = dict(os.environ)

        self.env.update(HOME=str(self.root / 'user'), USERPROFILE=str(self.root / 'user'),
                        XDG_CONFIG_HOME=str(self.root / 'user/.config'),
                        LOCALAPPDATA=str(self.root / 'local'), KRABB_PREFIX=str(self.root / 'prefix'),
                        KRABB_HOME=str(self.home), KRABB_KEEP_PATH='1')

        for name in ['KRABB_DB', 'KRABB_MEMORY', 'KRABB_ROOT', 'KRABB_MEDIA']:
            self.env.pop(name, None)

    def run_uninstaller(self, *args, **env):
        executable = shutil.which('pwsh' if WINDOWS else 'sh')

        if not executable:
            self.skipTest('uninstaller interpreter is unavailable')

        command = ([executable, '-NoProfile', '-NonInteractive', '-File'] if WINDOWS
                   else [executable])

        flags = [{'--purge': '-Purge', '--yes': '-Yes'}.get(arg, arg) for arg in args] if WINDOWS \
            else list(args)

        return subprocess.run([*command, str(UNINSTALLER), *flags], env={**self.env, **env},
                              stdin=subprocess.DEVNULL, capture_output=True, text=True,
                              timeout=30, start_new_session=True)

    def test_default_removes_software_and_keeps_data(self):
        result = self.run_uninstaller()

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

        for binary in self.binaries:
            self.assertFalse(binary.exists(), binary)

        for name in ['plugins', 'cache', 'updates/v0.2.0']:
            self.assertFalse((self.home / name).exists(), name)

        for name in ['config.toml', 'credentials.json', 'data/sessions/index.json',
                     'updates/recovery/point.json']:
            self.assertTrue((self.home / name).exists(), name)

        self.assertIn('with your settings, credentials, and conversations', result.stdout)
        self.assertIn('-Purge' if WINDOWS else '--purge', result.stdout)

    def test_purge_removes_everything_krabb_created_and_lists_the_rest(self):
        database = self.root / 'stores/krabb.db'
        memory = self.root / 'stores/memory.json'
        project = self.root / 'project'

        database.parent.mkdir()
        project.mkdir()

        for path in [database, memory, Path(f'{memory}.lock'), project / 'notes.md']:
            path.write_text('kept by the user' if path.parent == project else 'krabb')

        result = self.run_uninstaller('--purge', '--yes', KRABB_DB=str(database),
                                      KRABB_MEMORY=str(memory), KRABB_ROOT=str(project))

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(self.home.exists())

        for path in [database, memory, Path(f'{memory}.lock')]:
            self.assertFalse(path.exists(), path)

        self.assertTrue((project / 'notes.md').exists())
        self.assertIn(f'Kept {project} from KRABB_ROOT', result.stdout)

    def test_purge_without_confirmation_removes_nothing(self):
        result = self.run_uninstaller('--purge')

        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('-Yes' if WINDOWS else '--yes', result.stderr)

        for binary in self.binaries:
            self.assertTrue(binary.exists(), binary)

        self.assertTrue((self.home / 'config.toml').exists())

    def test_nothing_installed_says_so(self):
        shutil.rmtree(self.bin)
        shutil.rmtree(self.home)

        result = self.run_uninstaller()

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('Krabb was not found, so nothing was removed.', result.stdout)


if __name__ == '__main__':
    unittest.main()
