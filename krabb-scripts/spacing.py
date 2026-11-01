"""Repository code spacing; literals and existing blank lines remain intact."""

import ast
import re
import sys
import tokenize
from io import StringIO
from pathlib import Path


IGNORED = {"node_modules", "target", ".git", ".revloop", ".krabb", ".cache", ".target"}
LINE_LIMIT = 100
BLOCK_END = re.compile(r"^\}(?:\)|;|\);|\s*;)?$")
CONTINUATION = re.compile(r"^(?:\}|else\b|catch\b|finally\b|[.,;\)\]])")


def javascript_masks(lines):
    masks = []
    template_lines = []
    quote = None
    in_template = False
    in_block_comment = False

    for line in lines:
        mask = []
        started_in_template = in_template
        index = 0

        while index < len(line):
            char = line[index]
            following = line[index + 1] if index + 1 < len(line) else ""

            if in_template:
                if char == "\\":
                    index += 2
                    continue

                if char == "`":
                    in_template = False

                index += 1
                continue

            if quote:
                if char == "\\":
                    index += 2
                    continue

                if char == quote:
                    quote = None

                index += 1
                continue

            if in_block_comment:
                if char == "*" and following == "/":
                    in_block_comment = False

                    index += 2
                else:
                    index += 1

                continue

            if char == "/" and following == "/":
                break

            if char == "/" and following == "*":
                in_block_comment = True

                index += 2
                continue

            if char in "'\"":
                quote = char

                index += 1
                continue

            if char == "`":
                in_template = True

                index += 1
                continue

            mask.append(char)
            index += 1

        masks.append("".join(mask))
        template_lines.append(started_in_template)

        if quote:
            quote = None

    return masks, template_lines


def format_javascript(source):
    lines = source.splitlines()
    masks, template_lines = javascript_masks(lines)

    for number, line in enumerate(lines, 1):
        if len(line) > LINE_LIMIT:
            raise ValueError(f"line {number} exceeds the {LINE_LIMIT}-column limit.")

    for line, in_template in zip(lines, template_lines, strict=True):
        if line.strip() and not in_template:
            leading = len(line) - len(line.lstrip(" "))

            if "\t" in line[:leading]:
                raise ValueError("JavaScript indentation must use spaces.")

            if leading % 4:
                raise ValueError("JavaScript indentation must use four spaces per level.")

    output = []

    for index, line in enumerate(lines):
        output.append(line)

        if line.strip() and masks[index].strip() and BLOCK_END.fullmatch(masks[index].strip()):
            next_index = index + 1

            if next_index < len(lines):
                next_code = masks[next_index].strip()
                next_line = lines[next_index]
                continued = bool(CONTINUATION.match(next_code))

                if next_line.strip() and next_code and not continued:
                    if output and output[-1].strip():
                        output.append("")

    return "\n".join(output) + "\n"


def is_block_statement(statement):
    return isinstance(
        statement,
        (
            ast.AsyncFor,
            ast.AsyncFunctionDef,
            ast.AsyncWith,
            ast.ClassDef,
            ast.For,
            ast.FunctionDef,
            ast.If,
            ast.Match,
            ast.Try,
            getattr(ast, "TryStar", ast.Try),
            ast.While,
            ast.With,
        ),
    )


def python_phase(statement):
    if isinstance(statement, ast.Assert):
        return "assert"

    if isinstance(statement, ast.Expr) and isinstance(statement.value, ast.Call):
        function = statement.value.func

        if isinstance(function, ast.Attribute) and function.attr.startswith("assert"):
            return "assert"

    if isinstance(statement, (ast.Assign, ast.AnnAssign)):
        return "setup"

    return "action"


def separates_python(previous, current):
    if is_block_statement(previous) or is_block_statement(current):
        return True

    if any(statement.end_lineno > statement.lineno for statement in (previous, current)):
        return True

    return python_phase(previous) != python_phase(current)


def format_python(source):
    tree = ast.parse(source)
    lines = source.splitlines()
    boundaries = {}

    for number, line in enumerate(lines, 1):
        if len(line) > LINE_LIMIT:
            raise ValueError(f"line {number} exceeds the {LINE_LIMIT}-column limit.")

    def visit(node):
        for value in vars(node).values():
            if isinstance(value, list):
                statements = [item for item in value if isinstance(item, ast.stmt)]
                top_level = isinstance(node, ast.Module)

                for previous, current in zip(statements, statements[1:]):
                    if not separates_python(previous, current):
                        continue

                    definitions = (ast.AsyncFunctionDef, ast.ClassDef, ast.FunctionDef)

                    blank_lines = 2 if top_level and any(
                        isinstance(statement, definitions) for statement in (previous, current)
                    ) else 1

                    after_line = previous.end_lineno or previous.lineno
                    boundaries[after_line] = max(boundaries.get(after_line, 0), blank_lines)

                for item in value:
                    if isinstance(item, ast.AST):
                        visit(item)

            elif isinstance(value, ast.AST):
                visit(value)

    visit(tree)

    for token in tokenize.generate_tokens(StringIO(source).readline):
        if token.type == tokenize.INDENT:
            if "\t" in token.string or len(token.string) % 4:
                raise ValueError("Python indentation must use four spaces per level.")

    for after_line, blank_lines in sorted(boundaries.items(), reverse=True):
        index = after_line
        existing = 0

        while index + existing < len(lines) and not lines[index + existing].strip():
            existing += 1

        if existing < blank_lines:
            lines[index:index] = [""] * (blank_lines - existing)

    return "\n".join(lines).rstrip() + "\n"



def main():
    root = Path(sys.argv[1])
    check_only = sys.argv[2] == "true"
    status = 0

    paths = sorted(
        path
        for suffix in ("*.mjs", "*.py")
        for path in root.rglob(suffix)
        if not any(part in IGNORED for part in path.relative_to(root).parts)
    )

    for path in paths:
        original = path.read_text()

        try:
            formatter = format_javascript if path.suffix == ".mjs" else format_python
            formatted = formatter(original)
        except (SyntaxError, tokenize.TokenError, ValueError) as error:
            print(f"[ERROR] {path.relative_to(root)}: {error}", file=sys.stderr)

            status = 1

            continue

        if formatted == original:
            continue

        if check_only:
            warning = f"[WARN] Code spacing needs formatting: {path.relative_to(root)}"

            print(warning, file=sys.stderr)

            status = 1
        else:
            path.write_text(formatted)
            print(f"[FIXED] {path.relative_to(root)}")

    return status


if __name__ == "__main__":
    raise SystemExit(main())
