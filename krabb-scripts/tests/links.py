"""Markdown link checks report missing files and anchors, and nothing else."""

import importlib.util
from pathlib import Path
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('links', ROOT / 'krabb-scripts/links.py')
links = importlib.util.module_from_spec(SPEC)

SPEC.loader.exec_module(links)


class LinkTests(unittest.TestCase):
    def check(self, files):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            for name, text in files.items():
                path = root / name

                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text, encoding='utf-8')

            return links.broken(root)

    def test_resolving_links_pass(self):
        self.assertEqual(self.check({
            'a.md': '# A\n\n## Set Up `krabb`, Then Run\n\n[b](docs/b.md#usage) [self](#a)\n'
                    '[web](https://example.com/x.md) [mail](mailto:x@example.com)\n'
                    '[runs](#set-up-krabb-then-run) [dir](docs)\n',
            'docs/b.md': '# B\n\n## Usage\n\n[back](../a.md)\n',
        }), [])

    def test_missing_files_and_anchors_fail(self):
        self.assertEqual(self.check({
            'a.md': '# A\n\n[gone](gone.md)\n[anchor](b.md#nope)\n',
            'b.md': '# B\n',
        }), ['a.md:3: missing gone.md', 'a.md:4: missing anchor b.md#nope'])

    def test_repeated_headings_get_numbered_anchors(self):
        self.assertEqual(self.check({
            'a.md': '# A\n\n## Notes\n\n## Notes\n\n[second](#notes-1) [third](#notes-2)\n',
        }), ['a.md:7: missing anchor #notes-2'])

    def test_fenced_code_and_skipped_directories_are_ignored(self):
        self.assertEqual(self.check({
            'a.md': '# A\n\n```md\n[gone](gone.md)\n## Fake\n```\n\n[fake](#fake)\n',
            'target/b.md': '[gone](gone.md)\n',
        }), ['a.md:8: missing anchor #fake'])

    def test_the_repository_links_resolve(self):
        self.assertEqual(links.broken(ROOT), [])


if __name__ == '__main__':
    unittest.main()
