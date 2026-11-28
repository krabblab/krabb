"""Change selection tests exactly the packages a change can affect."""

import importlib.util
from pathlib import Path
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('changes', ROOT / 'krabb-scripts/changes.py')
changes = importlib.util.module_from_spec(SPEC)

SPEC.loader.exec_module(changes)

MEMBERS = {
    'krabb-core': 'krabb-core',
    'krabb-runtime': 'krabb-runtime',
    'krabb': 'krabb',
    'krabb-http': 'krabb-libs/http',
    'krabb-plugin-slack': 'krabb-plugins/channels/slack',
    'krabb-plugin-tools': 'krabb-plugins/tools',
}

DEPENDENCIES = {
    'krabb-core': set(),
    'krabb-http': {'krabb-core'},
    'krabb-runtime': {'krabb-core'},
    'krabb': {'krabb-runtime'},
    'krabb-plugin-slack': {'krabb-core', 'krabb-http'},
    'krabb-plugin-tools': {'krabb-core'},
}


def select(*changed):
    return changes.select(list(changed), MEMBERS, DEPENDENCIES)


class SelectTests(unittest.TestCase):
    def test_documentation_and_assets_need_no_code_checks(self):
        self.assertEqual(select(
            'README.md',
            'ROADMAP.md',
            'LICENSE',
            'krabb-docs/workflows.md',
            'krabb-handbook/release.md',
            'krabb-assets/brand/svg/krabb-mark-coral.svg',
            'krabb-plugins/channels/slack/README.md',
            'krabb-runtime/AGENTS.md',
        ), [])

    def test_a_package_selects_itself_and_its_dependents(self):
        self.assertEqual(select('krabb-runtime/src/daemon.rs'), ['krabb', 'krabb-runtime'])
        self.assertEqual(select('krabb-libs/http/src/lib.rs'), ['krabb-http', 'krabb-plugin-slack'])
        self.assertEqual(select('krabb-core/src/lib.rs'), sorted(MEMBERS))

    def test_a_leaf_plugin_selects_only_itself(self):
        self.assertEqual(select('krabb-plugins/tools/src/main.rs'), ['krabb-plugin-tools'])

    def test_a_plugin_manifest_also_selects_the_runtime_that_reads_it(self):
        self.assertEqual(
            select('krabb-plugins/channels/slack/krabb-plugin.toml'),
            ['krabb', 'krabb-plugin-slack', 'krabb-runtime'],
        )

    def test_markdown_that_a_crate_compiles_is_code(self):
        self.assertEqual(select('krabb-runtime/src/templates/KRAB.md'), ['krabb', 'krabb-runtime'])

    def test_shared_files_select_every_package(self):
        for path in (
            'Cargo.lock',
            'Cargo.toml',
            '.github/workflows/main.yml',
            'krabb-scripts/coverage.sh',
            'install.sh',
            'krabb-plugins/AGENTS.toml',
        ):
            with self.subTest(path=path):
                self.assertIsNone(select('README.md', path))

    def test_a_directory_prefix_is_not_a_package(self):
        self.assertIsNone(changes.owner('krabb-tools/src/main.rs', {'krabb': 'krabb'}))


class MainTests(unittest.TestCase):
    def test_without_a_base_every_package_runs(self):
        with mock.patch.dict('os.environ', {}, clear=True), \
                mock.patch('builtins.print') as printed:
            self.assertEqual(changes.main([]), 0)

        printed.assert_called_once_with('code=true\npackages=\ncargo=--workspace')

    def test_cargo_flags_follow_the_selection(self):
        self.assertEqual(changes.cargo(None), '--workspace')
        self.assertEqual(changes.cargo([]), '')
        self.assertEqual(changes.cargo(['krabb-core', 'krabb-http']), '-p krabb-core -p krabb-http')

    def test_local_changes_include_the_working_tree_and_new_files(self):
        replies = {
            ('merge-base', 'origin/main', 'HEAD'): ['fork'],
            ('diff', '--name-only', '--no-renames', 'fork'): ['krabb-core/src/lib.rs'],
            ('ls-files', '--others', '--exclude-standard'): ['krabb-libs/http/src/new.rs'],
        }

        with mock.patch.object(changes, 'git', side_effect=lambda *call: replies[call]):
            self.assertEqual(
                changes.changes('origin/main', local=True),
                ['krabb-core/src/lib.rs', 'krabb-libs/http/src/new.rs'],
            )

    def test_local_mode_needs_a_base(self):
        with mock.patch('builtins.print'):
            self.assertEqual(changes.main(['--local']), 2)

    def test_the_workspace_matches_its_members(self):
        members, dependencies = changes.workspace()

        self.assertEqual(members['krabb-plugin-slack'], 'krabb-plugins/channels/slack')
        self.assertIn('krabb-core', dependencies['krabb-runtime'])


if __name__ == '__main__':
    unittest.main()
