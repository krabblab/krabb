"""Formatting must separate phases without changing embedded programs or literals."""

import ast
import importlib.util
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SCRIPTS = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("spacing_engine", SCRIPTS / "spacing.py")
ENGINE = importlib.util.module_from_spec(SPEC)

SPEC.loader.exec_module(ENGINE)


class Spacing(unittest.TestCase):
    def test_python_phases_and_multiline_statements(self):
        source = '''def check():
    root = fixture()
    root.mkdir()
    result = run(
        root,
    )
    assert result.ok
    assert result.count == 1
    cleanup()
'''

        expected = '''def check():
    root = fixture()

    root.mkdir()

    result = run(
        root,
    )

    assert result.ok
    assert result.count == 1

    cleanup()
'''

        formatted = ENGINE.format_python(source)

        self.assertEqual(formatted, expected)
        self.assertEqual(ENGINE.format_python(formatted), formatted)
        self.assertEqual(ast.dump(ast.parse(source)), ast.dump(ast.parse(formatted)))

    def test_python_decorators_comments_and_fixtures(self):
        source = '''import unittest
class Example(unittest.TestCase):
    @decorate(
        "fixture",
    )
    def check(self):
        script = """let value = 1;
assert_eq!(value, 1);
if value > 0 {
    run();
}"""
        result = run(script)
        # Verify the result before cleanup.
        self.assertEqual(result, 1)
        self.assertTrue(result)
        cleanup()
'''

        formatted = ENGINE.format_python(source)

        self.assertEqual(ast.dump(ast.parse(source)), ast.dump(ast.parse(formatted)))
        self.assertIn('self.assertTrue(result)\n\n        cleanup()', formatted)
        self.assertIn('# Verify the result before cleanup.\n        self.assertEqual', formatted)
        self.assertEqual(ENGINE.format_python(formatted), formatted)

    def test_javascript_templates_and_continuations(self):
        source = '''if (ready) {
    run(`first
}
run();
last`);
} else {
    stop();
}
finish();
'''

        expected = source.replace('}\nfinish();', '}\n\nfinish();')
        formatted = ENGINE.format_javascript(source)

        self.assertEqual(formatted, expected)
        self.assertEqual(ENGINE.format_javascript(formatted), formatted)

    def test_check_mode_rust_literals_and_idempotence(self):
        source = '''fn check() {
    let root = fixture();
    root.create();
    let result = run(
        root,
    );
    assert!(result.ok());
    assert_eq!(result.count(), 1);
    cleanup();
    let plugins = Plugins::default();
    plugins.insert(process).await;
    let original = plugins.get("channel").await.unwrap();
    assert!(original.ready());
    save(&destination, values)
        .unwrap();
    let reloaded = read_manifest(&root).unwrap();
    let environment = env_for(&reloaded).unwrap();
    assert!(environment.contains(&expected));
    assert!(
        !environment.contains(&previous)
    );
    assert!(ready(&root));
    remove_dir_all(root).unwrap();
    let status = match result {
        Ready => {
            prepare();
            true
        },
        Pending => false,
    };
    let raw = r##"first
assert!(fixture);
if condition {
    run();
}
"##;
    let quoted = "first
assert!(fixture);
if condition {
    run();
}
last";
    let brace = '}';
    let escaped = '\\'';
    let unicode = '\\u{7d}';
    /* outer
    /* nested */
    assert!(fixture);
    if condition {
        run();
    }
    */
    consume(raw, quoted, brace, escaped, unicode);
}
'''

        with tempfile.TemporaryDirectory(prefix="krabb spacing ") as directory:
            root = Path(directory)
            scripts = root / "krabb-scripts"

            scripts.mkdir()

            for name in ("spacing.sh", "spacing.py"):
                shutil.copyfile(SCRIPTS / name, scripts / name)

            rust = root / "sample.rs"

            rust.write_text(source)

            command = ["bash", str(scripts / "spacing.sh")]

            checked = subprocess.run(
                command + ["--check"], capture_output=True, text=True, timeout=30
            )

            self.assertEqual(checked.returncode, 1, checked.stderr)
            self.assertEqual(rust.read_text(), source)

            changed = subprocess.run(
                command, capture_output=True, text=True, timeout=30
            )

            self.assertEqual(changed.returncode, 0, changed.stderr)

            formatted = rust.read_text()
            phases = 'let root = fixture();\n\n    root.create();\n\n    let result'

            self.assertIn(phases, formatted)
            self.assertIn('assert_eq!(result.count(), 1);\n\n    cleanup();', formatted)
            self.assertIn('        },\n\n        Pending => false,', formatted)

            self.assertIn(
                'let plugins = Plugins::default();\n\n'
                '    plugins.insert(process).await;\n\n'
                '    let original = plugins.get("channel").await.unwrap();',
                formatted,
            )

            self.assertIn('assert!(original.ready());\n\n    save(', formatted)

            self.assertIn(
                'let reloaded = read_manifest(&root).unwrap();\n\n'
                '    let environment = env_for(&reloaded).unwrap();',
                formatted,
            )

            self.assertIn('assert!(environment.contains(&expected));\n\n    assert!(\n', formatted)
            self.assertIn('assert!(ready(&root));\n\n    remove_dir_all(root).unwrap();', formatted)

            literals = [('r##"', '"##'), ('let quoted = "first', 'last"'), ('/* outer', '    */')]

            for start, end in literals:
                before = source[source.index(start):source.index(end) + len(end)]

                self.assertIn(before, formatted)

            self.assertEqual(source.split(), formatted.split())

            checked = subprocess.run(
                command + ["--check"], capture_output=True, text=True, timeout=30
            )

            self.assertEqual(checked.returncode, 0, checked.stderr)
            self.assertEqual(rust.read_text(), formatted)


if __name__ == "__main__":
    unittest.main()
