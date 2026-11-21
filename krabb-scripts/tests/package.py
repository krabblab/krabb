"""Packaging archives the named core or plugins, each at its own version."""

import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(os.name != 'nt' and shutil.which('bash'), 'requires Unix shell tools')
class Packaging(unittest.TestCase):
    def test_current_and_legacy_plugin_layouts_preserve_artifacts(self):
        with tempfile.TemporaryDirectory(prefix='krabb packaging ') as directory:
            root = Path(directory)
            scripts = root / 'krabb-scripts'
            commands = root / 'commands'
            target = 'x86_64-unknown-linux-gnu'
            release = root / 'target' / target / 'release'

            for path in [scripts, commands, release]:
                path.mkdir(parents=True)

            shutil.copyfile(ROOT / 'krabb-scripts/package.sh', scripts / 'package.sh')

            for name in ['Cargo.toml', 'LICENSE', 'NOTICE']:
                shutil.copyfile(ROOT / name, root / name)

            cargo = commands / 'cargo'

            cargo.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$KRABB_FIXTURE_CARGO"\n')
            cargo.chmod(0o755)

            manifests = {}

            for source in (ROOT / 'krabb-plugins').rglob('krabb-plugin.toml'):
                plugin = tomllib.loads(source.read_text())['id']
                destination = root / source.relative_to(ROOT)

                destination.parent.mkdir(parents=True)
                shutil.copyfile(source, destination)

                manifests[plugin] = source.read_bytes()

            for name in ['krabb', 'krabb-daemon', *['krabb-plugin-' + p for p in manifests]]:
                (release / name).write_text(name)

            calls = root / 'cargo-calls'

            env = dict(os.environ, PATH=str(commands) + os.pathsep + os.environ['PATH'],
                       KRABB_FIXTURE_CARGO=str(calls))

            for layout in ['current', 'legacy']:
                with self.subTest(layout=layout):
                    if layout == 'legacy':
                        (root / 'krabb-plugins/channels').rename(root / 'krabb-plugins/messaging')
                        (root / 'krabb-plugins/clients/tui').rename(root / 'krabb-plugins/tui')

                    result = subprocess.run(
                        ['bash', str(scripts / 'package.sh'), target, 'core', *manifests],
                        cwd=root, env=env, text=True, capture_output=True, check=False,
                    )

                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertEqual(len(list(root.glob('*.tar.gz'))), len(manifests) + 1)

                    for plugin, manifest in manifests.items():
                        version = tomllib.loads(manifest.decode())['version']
                        name = f'krabb-{plugin}-v{version}-{target}'

                        with tarfile.open(root / (name + '.tar.gz')) as archive:
                            self.assertEqual(
                                archive.extractfile(name + '/krabb-plugin.toml').read(), manifest,
                            )

                            binary = 'krabb-plugin-' + plugin

                            self.assertEqual(
                                archive.extractfile(name + '/bin/' + binary).read(),
                                binary.encode(),
                            )

            for archive in root.glob('*.tar.gz'):
                archive.unlink()

            workspace = tomllib.loads((ROOT / 'Cargo.toml').read_text())
            default = workspace['workspace']['package']['version']

            for version, expected in [(None, default), ('0.2.0-rc.1', '0.2.0-rc.1')]:
                build = {**env, 'KRABB_BUILD_VERSION': version or ''}

                result = subprocess.run(
                    ['bash', str(scripts / 'package.sh'), target],
                    cwd=root, env=build, text=True, capture_output=True, check=False,
                )

                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

                self.assertEqual([path.name for path in root.glob('*.tar.gz')],
                                 [f'krabb-core-v{expected}-{target}.tar.gz'])

                for archive in root.glob('*.tar.gz'):
                    archive.unlink()

            tools = next((root / 'krabb-plugins').rglob('tools/krabb-plugin.toml'))

            tools.write_text(re.sub(r'(?m)^version = ".*"$', 'version = "0.2.0"',
                                    tools.read_text(), count=1))

            result = subprocess.run(
                ['bash', str(scripts / 'package.sh'), target, 'tools'],
                cwd=root, env=env, text=True, capture_output=True, check=False,
            )

            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

            self.assertEqual([path.name for path in root.glob('*.tar.gz')],
                             [f'krabb-tools-v0.2.0-{target}.tar.gz'])

            self.assertIn('-p\nkrabb-plugin-tools\n', calls.read_text())
            self.assertNotIn('krabb-daemon', calls.read_text())

            for invalid in [['core', '../x'], ['missing']]:
                result = subprocess.run(
                    ['bash', str(scripts / 'package.sh'), target, *invalid],
                    cwd=root, env=env, text=True, capture_output=True, check=False,
                )

                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
