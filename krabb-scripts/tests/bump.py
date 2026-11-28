"""Version bumps pick changed components, raise versions, and roll changelogs."""

import importlib.util
import io
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('bump', ROOT / 'krabb-scripts/bump.py')
bump = importlib.util.module_from_spec(SPEC)

SPEC.loader.exec_module(bump)

CHANGELOG = '''# Changelog

## [Unreleased]

### Added

- Upcoming feature.

## [0.1.0] - 2026-11-01
'''


README = (
    "curl -fsSL https://example.invalid/install.sh | KRABB_VERSION=0.1.0-rc.1 sh\n"
    "$env:KRABB_VERSION = '0.1.0-rc.1'; irm https://example.invalid/install.ps1 | iex\n"
    "curl -fsSL https://example.invalid/releases/download/v0.1.0-rc.1/install.sh | sh\n"
    "irm https://example.invalid/releases/download/v0.1.0-rc.1/install.ps1 | iex\n"
    "curl -fsSL https://example.invalid/releases/latest/download/install.sh | sh\n"
)


def node(package, *dependencies):
    return {'id': package, 'deps': [{'pkg': dependency, 'dep_kinds': [{'kind': kind}]}
                                    for dependency, kind in dependencies]}


@unittest.skipUnless(shutil.which('git'), 'requires git')
class Bump(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix='krabb bump ')

        self.addCleanup(directory.cleanup)

        self.root = Path(directory.name)
        plugin = self.root / 'krabb-plugins/echo'

        plugin.mkdir(parents=True)
        (self.root / 'krabb').mkdir()

        (self.root / 'Cargo.toml').write_text(
            '[workspace]\nmembers = []\n\n[workspace.package]\nversion = "0.1.0"\n'
            'edition = "2024"\n',
        )

        (self.root / 'CITATION.cff').write_text('title: Krabb\nversion: 0.1.0\n')
        (self.root / 'README.md').write_text(README)
        (self.root / 'CHANGELOG.md').write_text(CHANGELOG)
        (self.root / 'krabb/main.rs').write_text('fn main() {}\n')
        (plugin / 'krabb-plugin.toml').write_text('id = "echo"\nversion = "0.1.0"\n\n[protocol]\n')

        (plugin / 'Cargo.toml').write_text(
            '[package]\nname = "krabb-plugin-echo"\nversion = "0.1.0"\n\n[dependencies]\n'
            'serde = { version = "1" }\n',
        )

        (plugin / 'main.rs').write_text('fn main() {}\n')
        self.lock('1.0.0')

        self.git('init', '-q')
        self.git('add', '.')

        self.git('-c', 'user.name=Test', '-c', 'user.email=test@example.com',
                 '-c', 'commit.gpgsign=false', 'commit', '-qm', 'initial')

        self.meta = {
            'packages': [
                {'id': 'krabb', 'name': 'krabb', 'source': None,
                 'manifest_path': str(self.root / 'krabb/Cargo.toml')},
                {'id': 'daemon', 'name': 'krabb-daemon', 'source': None,
                 'manifest_path': str(self.root / 'krabb/Cargo.toml')},
                {'id': 'echo', 'name': 'krabb-plugin-echo', 'source': None,
                 'manifest_path': str(plugin / 'Cargo.toml')},
                {'id': 'serde', 'name': 'serde', 'source': 'registry',
                 'manifest_path': '/registry/serde/Cargo.toml'},
                {'id': 'mock', 'name': 'mockall', 'source': 'registry',
                 'manifest_path': '/registry/mockall/Cargo.toml'},
            ],
            'resolve': {'nodes': [
                node('krabb'), node('daemon'), node('serde'), node('mock'),
                node('echo', ('serde', None), ('mock', 'dev')),
            ]},
        }

    def git(self, *arguments):
        subprocess.run(['git', *arguments], cwd=self.root, check=True, capture_output=True)

    def lock(self, serde, mockall='1.0.0', core='0.1.0'):
        (self.root / 'Cargo.lock').write_text(
            f'version = 4\n\n[[package]]\nname = "krabb-core"\nversion = "{core}"\n\n'
            f'[[package]]\nname = "serde"\nversion = "{serde}"\n'
            'source = "registry"\n\n[[package]]\nname = "mockall"\n'
            f'version = "{mockall}"\nsource = "registry"\n',
        )

    def run_bump(self, *arguments):
        output = io.StringIO()

        with mock.patch.object(bump, 'metadata', return_value=self.meta), \
                mock.patch('sys.stdout', output), mock.patch('sys.stderr', output):
            code = bump.main(list(arguments), self.root)

        return code, output.getvalue()

    def changed(self):
        found = bump.components(self.root)

        return [name for name, component in found.items()
                if bump.changed(component, self.meta, self.root)]

    def test_versions_follow_promotion_patch_and_candidate_rules(self):
        for current, unstable, requested, expected in [
            ('0.1.0', False, None, '0.1.1'),
            ('0.1.0', True, None, '0.1.1-rc.1'),
            ('0.1.1-rc.2', True, None, '0.1.1-rc.3'),
            ('0.1.1-rc.2', False, None, '0.1.1'),
            ('0.1.0', False, '0.2.0', '0.2.0'),
            ('0.1.0', True, '0.2.0', '0.2.0-rc.1'),
            ('0.2.0-rc.1', True, '0.2.0', '0.2.0-rc.2'),
        ]:
            with self.subTest(current=current, unstable=unstable, requested=requested):
                self.assertEqual(bump.following(current, unstable, requested), expected)

        for current, requested, message in [('0.2.0', '0.1.0', 'not newer than 0.2.0'),
                                            ('0.1.0', '0.2.0-rc.1', 'Pass --version as X.Y.Z'),
                                            ('main', None, 'is not X.Y.Z')]:
            with self.subTest(current=current, requested=requested):
                with self.assertRaisesRegex(bump.Failure, message):
                    bump.following(current, False, requested)

    def test_only_components_with_changed_sources_or_shipped_locks_are_selected(self):
        self.assertEqual(self.changed(), [])

        self.lock('1.0.0', mockall='2.0.0')

        self.assertEqual(self.changed(), [])

        self.lock('1.0.1')

        self.assertEqual(self.changed(), ['echo'])

        self.lock('1.0.0', core='0.2.0')

        self.assertEqual(self.changed(), [])

        cargo = self.root / 'Cargo.toml'

        cargo.write_text(cargo.read_text().replace('version = "0.1.0"', 'version = "0.1.0"', 1))

        self.assertEqual(self.changed(), [])

        cargo.write_text(cargo.read_text() + '\n[profile.release]\nlto = true\n')

        self.assertEqual(self.changed(), ['krabb', 'echo'])

        self.git('checkout', '--', 'Cargo.toml')
        self.lock('1.0.0')
        (self.root / 'krabb/main.rs').write_text('fn main() { }\n')

        self.assertEqual(self.changed(), ['krabb'])

        self.git('checkout', '--', 'krabb/main.rs')
        (self.root / 'krabb-plugins/echo/extra.rs').write_text('fn extra() {}\n')

        self.assertEqual(self.changed(), ['echo'])

    def test_stable_bumps_set_every_version_file_and_roll_the_changelog(self):
        (self.root / 'krabb-plugins/echo/main.rs').write_text('fn main() { }\n')

        code, output = self.run_bump('--yes')

        self.assertEqual(code, 0, output)
        self.assertIn('echo: 0.1.0 -> 0.1.1', output)

        manifest = (self.root / 'krabb-plugins/echo/krabb-plugin.toml').read_text()
        crate = (self.root / 'krabb-plugins/echo/Cargo.toml').read_text()

        self.assertIn('version = "0.1.1"\n\n[protocol]', manifest)
        self.assertIn('version = "0.1.1"\n\n[dependencies]', crate)
        self.assertIn('serde = { version = "1" }', crate)

        code, output = self.run_bump('krabb', '--version', '0.2.0', '--yes')

        self.assertEqual(code, 0, output)

        cargo = (self.root / 'Cargo.toml').read_text()
        changelog = (self.root / 'CHANGELOG.md').read_text()

        self.assertIn('[workspace.package]\nversion = "0.2.0"\n', cargo)
        self.assertIn('version: 0.2.0\n', (self.root / 'CITATION.cff').read_text())
        readme = (self.root / 'README.md').read_text()

        self.assertEqual(readme, README.replace('0.1.0-rc.1', '0.2.0'))

        self.assertRegex(changelog, r'## \[Unreleased\]\n\n## \[0\.2\.0\] - \d{4}-\d{2}-\d{2}\n'
                                    r'\n### Added\n\n- Upcoming feature\.')

    def test_changelog_sections_gain_a_date_and_missing_notes_are_reported(self):
        changelog = self.root / 'CHANGELOG.md'

        changelog.write_text('# Changelog\n\n## [0.2.0]\n\n- Feature.\n')

        self.assertTrue(bump.roll(changelog, '0.2.0', '2026-12-01'))
        self.assertIn('## [0.2.0] - 2026-12-01\n', changelog.read_text())
        self.assertFalse(bump.roll(changelog, '0.3.0', '2026-12-01'))
        self.assertFalse(bump.roll(self.root / 'missing.md', '0.3.0', '2026-12-01'))

        code, output = self.run_bump('krabb', '--version', '0.3.0', '--yes')

        self.assertEqual(code, 0, output)
        self.assertIn('CHANGELOG.md has no notes for 0.3.0', output)

        code, output = self.run_bump('krabb', '--unstable', '--yes')

        self.assertEqual(code, 0, output)
        self.assertNotIn('no notes', output)
        self.assertIn('version = "0.3.1-rc.1"', (self.root / 'Cargo.toml').read_text())

    def test_bumps_need_confirmation_and_report_empty_plans(self):
        code, output = self.run_bump('echo')

        self.assertEqual(code, 2)
        self.assertIn('Confirm the bumps in a terminal', output)

        crate = self.root / 'krabb-plugins/echo/Cargo.toml'

        self.assertIn('version = "0.1.0"', crate.read_text())

        with mock.patch('sys.stdin', mock.Mock(isatty=lambda: True)), \
                mock.patch('builtins.input', return_value='n'):
            code, output = self.run_bump('echo')

        self.assertEqual(code, 1)
        self.assertIn('No version was changed.', output)

        with mock.patch('sys.stdin', mock.Mock(isatty=lambda: True)), \
                mock.patch('builtins.input', return_value='y'):
            code, output = self.run_bump('echo', '--unstable')

        self.assertEqual(code, 0, output)
        self.assertIn('version = "0.1.1-rc.1"', crate.read_text())

        code, output = self.run_bump()

        self.assertEqual(code, 0)
        self.assertIn('Nothing changed since the current versions', output)

        code, output = self.run_bump('nope')

        self.assertEqual(code, 2)
        self.assertIn('Unknown component nope', output)

    def test_a_krabb_bump_warns_when_core_changed_but_the_protocol_did_not(self):
        types = self.root / bump.TYPES
        current = 'pub const CURRENT: Self = Self { major: 0, minor: 2 };\n'

        types.parent.mkdir(parents=True)
        types.write_text(current)
        self.git('add', '.')

        self.git('-c', 'user.name=Test', '-c', 'user.email=test@example.com',
                 '-c', 'commit.gpgsign=false', 'commit', '-q', '--amend', '--no-edit')

        krabb = bump.components(self.root)['krabb']
        bumps = [(krabb, '0.1.1')]

        self.assertIsNone(bump.unchanged_protocol(bumps, self.root))

        types.write_text(current + 'pub struct Added;\n')

        warning = bump.unchanged_protocol(bumps, self.root)

        self.assertIn('the plugin protocol is still 0.2', warning)
        self.assertIsNone(bump.unchanged_protocol([], self.root))

        types.write_text(current.replace('minor: 2', 'minor: 3'))

        self.assertIsNone(bump.unchanged_protocol(bumps, self.root))

        types.write_text('pub const CURRENT: Protocol = Protocol::new(0, 3);\n')

        self.assertIn('update bump.py', bump.unchanged_protocol(bumps, self.root))

    def test_repository_components_are_discovered(self):
        found = bump.components()

        self.assertEqual(found['krabb'].packages, ['krabb', 'krabb-daemon'])
        self.assertEqual(found['tools'].packages, ['krabb-plugin-tools'])


if __name__ == '__main__':
    unittest.main(verbosity=2)
