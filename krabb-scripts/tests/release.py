"""Release tags pick their version, pre-release state, and notes in one place."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]

CHANGELOG = '''# Changelog

## [0.2.0] - 2026-12-01

### Added

- Dated feature.

## [0.1.0]

### Added

- First feature.

## [0.0.9]
'''


@unittest.skipUnless(os.name != 'nt' and shutil.which('bash'), 'requires a Unix shell')
class Release(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix='krabb release ')

        self.addCleanup(directory.cleanup)

        self.root = Path(directory.name)
        self.notes = self.root / 'notes.md'

        (self.root / 'krabb-scripts').mkdir()
        shutil.copyfile(ROOT / 'krabb-scripts/release.sh', self.root / 'krabb-scripts/release.sh')
        (self.root / 'CHANGELOG.md').write_text(CHANGELOG)
        self.workspace('0.1.0')

    def workspace(self, version):
        (self.root / 'Cargo.toml').write_text(f'[workspace.package]\nversion = "{version}"\n')

    def notes_for(self, tag):
        return subprocess.run(
            ['bash', str(self.root / 'krabb-scripts/release.sh'), '--notes', tag, str(self.notes)],
            cwd=self.root, capture_output=True, text=True, check=False,
        )

    def test_stable_notes_come_from_the_changelog_with_a_dated_headline(self):
        result = self.notes_for('v0.1.0')

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, '0.1.0 false\n')

        lines = self.notes.read_text().splitlines()

        self.assertRegex(lines[0], r'^## krabb@v0\.1\.0 \| \d{4}-\d{2}-\d{2}$')
        self.assertEqual(lines[1:], ['', '### Added', '', '- First feature.', ''])

        self.workspace('0.2.0')

        result = self.notes_for('v0.2.0')

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.notes.read_text().splitlines()[0], '## krabb@v0.2.0 | 2026-12-01')

    def test_pre_releases_need_no_changelog_but_match_the_workspace(self):
        self.workspace('0.3.0')

        for tag in ['v0.3.0-rc.1', 'v0.3.0.rc1']:
            with self.subTest(tag=tag):
                result = self.notes_for(tag)

                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, f'{tag[1:]} true\n')

                notes = self.notes.read_text()

                self.assertTrue(notes.startswith(f'Krabb {tag[1:]} is a pre-release'), notes)

    def test_stable_tags_need_a_matching_workspace_and_changelog_notes(self):
        for tag, message in [('v0.2.0', "does not match the workspace version '0.1.0'"),
                             ('v0.0.1-rc.5', "does not match the workspace version '0.1.0'"),
                             ('main', 'is not a valid release tag')]:
            with self.subTest(tag=tag):
                result = self.notes_for(tag)

                self.assertEqual(result.returncode, 2)
                self.assertIn(message, result.stderr)

        self.workspace('0.0.9')

        result = self.notes_for('v0.0.9')

        self.assertEqual(result.returncode, 2)
        self.assertIn("CHANGELOG.md has no notes for version '0.0.9'", result.stderr)


if __name__ == '__main__':
    unittest.main()
