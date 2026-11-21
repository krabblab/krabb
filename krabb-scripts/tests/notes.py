"""Release notes follow the workspace version, its changelog section, or a default."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]

CHANGELOG = '''# Changelog

## [Unreleased]

### Added

- Upcoming feature.

## [0.2.0] - 2026-12-01

### Added

- Dated feature.

## [0.1.0]

### Added

- First feature.

## [0.0.9]
'''


@unittest.skipUnless(os.name != 'nt' and shutil.which('bash'), 'requires a Unix shell')
class Notes(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix='krabb notes ')

        self.addCleanup(directory.cleanup)

        self.root = Path(directory.name)
        self.notes = self.root / 'notes.md'

        (self.root / 'krabb-scripts').mkdir()

        for script in ['notes.sh', 'version.sh']:
            shutil.copyfile(ROOT / 'krabb-scripts' / script, self.root / 'krabb-scripts' / script)

        (self.root / 'CHANGELOG.md').write_text(CHANGELOG)

    def notes_for(self, version):
        (self.root / 'Cargo.toml').write_text(
            f'[workspace]\nmembers = []\n\n[workspace.package]\nversion = "{version}"\n',
        )

        return subprocess.run(
            ['bash', str(self.root / 'krabb-scripts/notes.sh'), str(self.notes)],
            cwd=self.root, capture_output=True, text=True, check=False,
        )

    def test_stable_notes_come_from_the_changelog_with_a_dated_headline(self):
        result = self.notes_for('0.1.0')

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, '0.1.0 false\n')

        lines = self.notes.read_text().splitlines()

        self.assertRegex(lines[0], r'^## krabb@v0\.1\.0 \| \d{4}-\d{2}-\d{2}$')
        self.assertEqual(lines[1:], ['', '### Added', '', '- First feature.', ''])

        result = self.notes_for('0.2.0')

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.notes.read_text().splitlines()[0], '## krabb@v0.2.0 | 2026-12-01')

    def test_stable_releases_without_changelog_notes_use_a_default_description(self):
        for version in ['0.0.9', '0.3.0']:
            with self.subTest(version=version):
                result = self.notes_for(version)

                self.assertEqual(result.returncode, 0, result.stderr)

                lines = self.notes.read_text().splitlines()

                self.assertRegex(lines[0], rf'^## krabb@v{version} \| \d{{4}}-\d{{2}}-\d{{2}}$')
                self.assertTrue(lines[2].startswith(f'Krabb {version} is a stable release.'))
                self.assertTrue(lines[-1].endswith('with `krabb update`.'), lines)

    def test_pre_releases_need_no_changelog(self):
        for version in ['0.3.0-rc.1', '0.3.0.rc1']:
            with self.subTest(version=version):
                result = self.notes_for(version)

                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, f'{version} true\n')

                notes = self.notes.read_text()

                self.assertTrue(notes.startswith(f'Krabb {version} is a pre-release'), notes)

    def test_invalid_workspace_versions_are_refused(self):
        result = self.notes_for('main')

        self.assertEqual(result.returncode, 2)
        self.assertIn("Workspace version 'main' is not a valid release version.", result.stderr)


if __name__ == '__main__':
    unittest.main()
