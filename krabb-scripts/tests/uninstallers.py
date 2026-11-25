"""The uninstall scripts remove Krabb software by default and everything only on --purge."""

import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
WINDOWS = os.name == 'nt'
MACOS = platform.system() == 'Darwin'
UNINSTALLER = ROOT / ('uninstall.ps1' if WINDOWS else 'uninstall.sh')
KEY = 'a' * 64

# Every tool that reaches a service manager, keyring, or process table is replaced by a stub
# that records its arguments, so no test can touch the developer's real installation.
TOOLS = ['systemctl', 'launchctl', 'security', 'secret-tool', 'pkill']


class Uninstallers(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='krabb uninstaller ')

        self.addCleanup(self.directory.cleanup)

        self.root = Path(self.directory.name)
        extension = '.exe' if WINDOWS else ''

        self.bin = self.root / ('user/AppData/Local/Krabb/bin' if WINDOWS else 'prefix/bin')
        self.home = self.root / 'krabb-home'
        self.binaries = [self.bin / ('krabb' + extension), self.bin / ('krabb-daemon' + extension)]
        self.calls = self.root / 'calls'
        stubs = self.root / 'stubs'

        self.bin.mkdir(parents=True)
        stubs.mkdir()

        for binary in self.binaries:
            binary.write_text('binary')

        for tool in TOOLS:
            stub = stubs / tool

            stub.write_text(f'#!/bin/sh\nprintf "%s\\n" "{tool} $*" >> "{self.calls}"\nexit 1\n')
            stub.chmod(0o755)

        for name in ['plugins/echo/bin/krabb-plugin-echo', 'cache/index.json',
                     'updates/v0.2.0/krabb.tar.gz', 'updates/recovery/point.json',
                     'config.toml', 'plugins.lock', 'data/sessions/index.json']:
            self.write(self.home / name, name)

        self.write(self.home / 'credentials.json', json.dumps({
            'format': 'krabb.credentials.v1', 'backend': 'keyring', 'key_id': KEY,
            'nonce': '', 'ciphertext': ''}))

        self.env = {name: value for name, value in os.environ.items()
                    if not name.startswith('KRABB_') and name != 'XDG_CONFIG_HOME'}

        self.env.update(HOME=str(self.root / 'user'), USERPROFILE=str(self.root / 'user'),
                        XDG_CONFIG_HOME=str(self.root / 'user/.config'),
                        LOCALAPPDATA=str(self.root / 'user/AppData/Local'),
                        KRABB_PREFIX=str(self.root / 'prefix'), KRABB_HOME=str(self.home),
                        KRABB_KEEP_PATH='1', PATH=f'{stubs}{os.pathsep}{os.environ["PATH"]}')

    def write(self, path, content):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    def recorded(self):
        return self.calls.read_text() if self.calls.exists() else ''

    def run_uninstaller(self, *args, **env):
        executable = shutil.which('pwsh' if WINDOWS else 'sh')

        if not executable:
            self.skipTest('uninstaller interpreter is unavailable')

        command = ([executable, '-NoProfile', '-NonInteractive', '-File'] if WINDOWS
                   else [executable])

        names = {'--purge': '-Purge', '--yes': '-Yes'}
        flags = [names.get(arg, arg) for arg in args] if WINDOWS else list(args)

        return subprocess.run([*command, str(UNINSTALLER), *flags], env={**self.env, **env},
                              stdin=subprocess.DEVNULL, capture_output=True, text=True,
                              timeout=30, start_new_session=not WINDOWS)

    def service(self, daemon, environment):
        if MACOS:
            path = self.root / 'user/Library/LaunchAgents/it.airscript.krabb.plist'
            variables = ''.join(f'<key>{name}</key><string>{value}</string>'
                                for name, value in environment.items())
            text = ('<plist version="1.0"><dict><key>Label</key><string>it.airscript.krabb'
                    f'</string><key>ProgramArguments</key><array><string>{daemon}</string>'
                    f'</array><key>EnvironmentVariables</key><dict>{variables}</dict></dict>'
                    '</plist>\n')
        else:
            path = self.root / 'user/.config/systemd/user/krabb.service'
            variables = ''.join(f'Environment="{name}={value}"\n'
                                for name, value in environment.items())
            text = f'[Service]\nExecStart="{daemon}"\n{variables}Restart=always\n'

        self.write(path, text)
        return path

    def test_default_removes_software_and_keeps_data(self):
        result = self.run_uninstaller()

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

        for binary in self.binaries:
            self.assertFalse(binary.exists(), binary)

        for name in ['cache', 'updates/v0.2.0']:
            self.assertFalse((self.home / name).exists(), name)

        for name in ['plugins/echo/bin/krabb-plugin-echo', 'config.toml', 'credentials.json',
                     'data/sessions/index.json', 'updates/recovery/point.json']:
            self.assertTrue((self.home / name).exists(), name)

        self.assertIn('with your plugins, settings, credentials, and conversations',
                      result.stdout)

        self.assertIn('-Purge' if WINDOWS else '--purge', result.stdout)
        self.assertNotIn('security', self.recorded())
        self.assertNotIn('secret-tool', self.recorded())

    def test_purge_removes_everything_krabb_created_and_lists_the_rest(self):
        stores = self.root / 'stores'
        project = self.root / 'project'
        database = stores / 'krabb.db'
        memory = stores / 'memory.json'
        created = [database, stores / 'krabb.db-wal', memory, stores / 'memory.lock',
                   stores / '.memory.json.tmp-1']

        for path in [*created, project / 'notes.md']:
            self.write(path, 'data')

        result = self.run_uninstaller('--purge', '--yes', KRABB_DB=str(database),
                                      KRABB_MEMORY=str(memory), KRABB_ROOT=str(project))

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(self.home.exists())

        for path in created:
            self.assertFalse(path.exists(), path)

        self.assertTrue((project / 'notes.md').exists())
        self.assertIn(str(project), result.stdout)

        if not WINDOWS:
            tool = 'security' if MACOS else 'secret-tool'
            calls = [line for line in self.recorded().splitlines() if line.startswith(tool)]

            self.assertEqual(len(calls), 1, self.recorded())
            self.assertIn(f'credentials:{KEY}', calls[0])

    def test_purge_without_confirmation_removes_nothing(self):
        result = self.run_uninstaller('--purge')

        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('-Yes' if WINDOWS else '--yes', result.stderr)

        for binary in self.binaries:
            self.assertTrue(binary.exists(), binary)

        self.assertTrue((self.home / 'config.toml').exists())

    def test_a_folder_that_is_not_a_krabb_home_is_never_deleted(self):
        folder = self.root / 'user'

        self.write(folder / 'cache/mine.txt', 'mine')

        result = self.run_uninstaller('--purge', '--yes', KRABB_HOME=str(folder))

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue((folder / 'cache/mine.txt').exists())
        self.assertIn('does not look like a Krabb home', result.stdout)

    def test_nothing_installed_says_so(self):
        shutil.rmtree(self.bin)
        shutil.rmtree(self.home)

        result = self.run_uninstaller()

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('Krabb was not found, so nothing was removed.', result.stdout)

    @unittest.skipIf(WINDOWS, 'Windows registers one machine-wide service')
    def test_the_service_of_another_krabb_is_kept(self):
        other = self.root / 'elsewhere/bin/krabb-daemon'
        definition = self.service(other, {'KRABB_HOME': str(self.home)})

        result = self.run_uninstaller('--purge', '--yes')

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(definition.exists())
        self.assertIn(f'the service of the Krabb at {other}', result.stdout)
        self.assertNotIn('disable', self.recorded())
        self.assertNotIn('bootout', self.recorded())

    @unittest.skipIf(WINDOWS, 'Windows reads the service from the registry')
    def test_the_installed_service_names_the_home_to_purge(self):
        served = self.root / 'served-home'

        self.write(served / 'config.toml', 'served')

        definition = self.service(self.binaries[1], {'KRABB_HOME': str(served)})
        self.env.pop('KRABB_HOME')

        result = self.run_uninstaller('--purge', '--yes')

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(definition.exists())
        self.assertFalse(served.exists())
        self.assertTrue((self.home / 'config.toml').exists())


if __name__ == '__main__':
    unittest.main()
