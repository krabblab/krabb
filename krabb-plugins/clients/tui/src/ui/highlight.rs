use std::ops::Range;

use ratatui::style::{Color, Modifier, Style};

/// What a piece of code is, so it can be colored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Token {
    Plain,
    Keyword,
    Text,
    Comment,
    Number,
    Added,
    Removed,
    Hunk,
    Header,
}

/// A language the highlighter knows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Language {
    Rust,
    Python,
    Script,
    Shell,
    Json,
    Toml,
    Yaml,
    Go,
    Diff,
}

struct Grammar {
    keywords: &'static [&'static str],
    comment: &'static str,
    block: Option<(&'static str, &'static str)>,
    quotes: &'static [&'static str],
}

const RUST: Grammar = Grammar {
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
        "true", "type", "unsafe", "use", "where", "while",
    ],
    comment: "//",
    block: Some(("/*", "*/")),
    quotes: &["\""],
};

const PYTHON: Grammar = Grammar {
    keywords: &[
        "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class",
        "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global",
        "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return",
        "try", "while", "with", "yield",
    ],
    comment: "#",
    block: None,
    quotes: &["\"\"\"", "'''", "\"", "'"],
};

const SCRIPT: Grammar = Grammar {
    keywords: &[
        "as", "async", "await", "break", "case", "catch", "class", "const", "continue", "default",
        "delete", "else", "enum", "export", "extends", "false", "finally", "for", "from",
        "function", "if", "implements", "import", "in", "instanceof", "interface", "let", "new",
        "null", "of", "private", "public", "readonly", "return", "static", "super", "switch",
        "this", "throw", "true", "try", "type", "typeof", "undefined", "var", "void", "while",
        "yield",
    ],
    comment: "//",
    block: Some(("/*", "*/")),
    quotes: &["`", "\"", "'"],
};

const SHELL: Grammar = Grammar {
    keywords: &[
        "case", "do", "done", "elif", "else", "esac", "exit", "export", "fi", "for", "function",
        "if", "in", "local", "readonly", "return", "set", "then", "until", "while",
    ],
    comment: "#",
    block: None,
    quotes: &["\"", "'"],
};

const JSON: Grammar =
    Grammar { keywords: &["false", "null", "true"], comment: "", block: None, quotes: &["\""] };

const TOML: Grammar = Grammar {
    keywords: &["false", "true"],
    comment: "#",
    block: None,
    quotes: &["\"\"\"", "'''", "\"", "'"],
};

const YAML: Grammar = Grammar {
    keywords: &["false", "no", "null", "true", "yes"],
    comment: "#",
    block: None,
    quotes: &["\"", "'"],
};

const GO: Grammar = Grammar {
    keywords: &[
        "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough",
        "false", "for", "func", "go", "goto", "if", "import", "interface", "iota", "map", "nil",
        "package", "range", "return", "select", "struct", "switch", "true", "type", "var",
    ],
    comment: "//",
    block: Some(("/*", "*/")),
    quotes: &["`", "\"", "'"],
};

impl Language {
    /// The language a code block's label names, if the highlighter knows it.
    pub(super) fn named(label: &str) -> Option<Self> {
        let language = match label.to_ascii_lowercase().as_str() {
            "rust" | "rs" => Self::Rust,
            "python" | "py" | "python3" => Self::Python,
            "javascript" | "js" | "jsx" | "mjs" | "cjs" | "typescript" | "ts" | "tsx" => {
                Self::Script
            }
            "sh" | "bash" | "shell" | "zsh" | "console" | "shellsession" => Self::Shell,
            "json" | "jsonc" | "json5" => Self::Json,
            "toml" => Self::Toml,
            "yaml" | "yml" => Self::Yaml,
            "go" | "golang" => Self::Go,
            "diff" | "patch" | "udiff" => Self::Diff,
            _ => return None,
        };

        Some(language)
    }

    /// The language of a block labeled LABEL whose lines are LINES: a known label, or a diff
    /// when an unlabeled block reads as a unified diff.
    pub(super) fn detect(label: &str, lines: &[&str]) -> Option<Self> {
        if let Some(language) = Self::named(label) {
            return Some(language);
        }

        let hunk = lines.iter().any(|line| line.starts_with("@@ "));
        let header = lines.iter().any(|line| line.starts_with("--- ") || line.starts_with("+++ "));

        (hunk && header).then_some(Self::Diff)
    }

    fn grammar(self) -> &'static Grammar {
        match self {
            Self::Rust => &RUST,
            Self::Python => &PYTHON,
            Self::Script => &SCRIPT,
            Self::Shell | Self::Diff => &SHELL,
            Self::Json => &JSON,
            Self::Toml => &TOML,
            Self::Yaml => &YAML,
            Self::Go => &GO,
        }
    }
}

/// What a line leaves open for the next one: a block comment or a multi-line string.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct State {
    open: Option<(&'static str, Token)>,
}

/// The tokens of one LINE, given what earlier lines left open in STATE, which it updates.
pub(super) fn line(language: Language, state: &mut State, line: &str) -> Vec<(Range<usize>, Token)> {
    if language == Language::Diff {
        return vec![(0..line.len(), diff(line))];
    }

    let grammar = language.grammar();
    let bytes = line.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;

    if let Some((close, token)) = state.open {
        match line.find(close) {
            Some(end) => {
                index = end + close.len();
                state.open = None;
            }

            None => index = line.len(),
        }

        tokens.push((0..index, token));
    }

    if index == 0 && matches!(language, Language::Toml | Language::Yaml) {
        index = key(language, line, &mut tokens);
    }

    while index < line.len() {
        let rest = &line[index..];
        let previous = line[..index].chars().next_back();

        if !grammar.comment.is_empty()
            && rest.starts_with(grammar.comment)
            && (grammar.comment != "#" || previous.is_none_or(char::is_whitespace))
        {
            tokens.push((index..line.len(), Token::Comment));
            break;
        }

        if let Some((open, close)) = grammar.block
            && rest.starts_with(open)
        {
            let start = index;

            match rest[open.len()..].find(close) {
                Some(end) => index += open.len() + end + close.len(),

                None => {
                    state.open = Some((close, Token::Comment));
                    index = line.len();
                }
            }

            tokens.push((start..index, Token::Comment));
            continue;
        }

        if let Some(quote) = grammar.quotes.iter().find(|quote| rest.starts_with(**quote)) {
            let start = index;
            let end = closing(&rest[quote.len()..], quote);

            match end {
                Some(end) => index += quote.len() + end,

                None if quote.len() == 3 || *quote == "`" => {
                    state.open = Some((*quote, Token::Text));
                    index = line.len();
                }

                None => index = line.len(),
            }

            tokens.push((start..index, Token::Text));
            continue;
        }

        if language == Language::Rust
            && rest.starts_with('\'')
            && let Some(end) = character(rest)
        {
            tokens.push((index..index + end, Token::Text));
            index += end;
            continue;
        }

        let character = rest.chars().next().unwrap_or_default();
        let joined = previous.is_some_and(|previous| previous.is_alphanumeric() || previous == '_');

        if character.is_ascii_digit() && !joined {
            let end = rest
                .find(|next: char| !(next.is_ascii_alphanumeric() || matches!(next, '_' | '.')))
                .unwrap_or(rest.len());

            tokens.push((index..index + end, Token::Number));
            index += end;
            continue;
        }

        if (character.is_alphabetic() || character == '_') && !joined {
            let end = rest
                .find(|next: char| !(next.is_alphanumeric() || next == '_'))
                .unwrap_or(rest.len());

            if grammar.keywords.contains(&&rest[..end]) {
                tokens.push((index..index + end, Token::Keyword));
            }

            index += end;
            continue;
        }

        index += character.len_utf8().max(1);

        while index < bytes.len() && !line.is_char_boundary(index) {
            index += 1;
        }
    }

    tokens
}

/// How TOKEN looks: terminal palette colors, which follow light and dark themes, and text
/// styles that still tell tokens apart without color.
pub(super) fn style(token: Token) -> Style {
    let plain = Style::default().fg(Color::Reset);

    match token {
        Token::Plain => plain,
        Token::Keyword => Style::default().fg(super::ACCENT_COLOR).add_modifier(Modifier::BOLD),
        Token::Text | Token::Added => Style::default().fg(Color::Green),
        Token::Comment => Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
        Token::Number => Style::default().fg(Color::Magenta),
        Token::Removed => Style::default().fg(Color::Red),
        Token::Hunk => Style::default().fg(super::ACCENT_COLOR),
        Token::Header => plain.add_modifier(Modifier::BOLD),
    }
}

/// LINE split into styled pieces that cover all of it, plain when LANGUAGE is unknown.
pub(super) fn pieces(
    language: Option<Language>,
    state: &mut State,
    text: &str,
) -> Vec<(Range<usize>, Style)> {
    let plain = style(Token::Plain);

    let Some(language) = language else {
        return vec![(0..text.len(), plain)];
    };

    let mut pieces = Vec::new();
    let mut offset = 0;

    for (range, token) in line(language, state, text) {
        if range.start > offset {
            pieces.push((offset..range.start, plain));
        }

        offset = range.end;
        pieces.push((range, style(token)));
    }

    if offset < text.len() || pieces.is_empty() {
        pieces.push((offset..text.len(), plain));
    }

    pieces
}

fn diff(line: &str) -> Token {
    if line.starts_with("+++ ") || line.starts_with("--- ") || line.starts_with("diff ") {
        Token::Header
    } else if line.starts_with("@@") {
        Token::Hunk
    } else if line.starts_with('+') {
        Token::Added
    } else if line.starts_with('-') {
        Token::Removed
    } else {
        Token::Plain
    }
}

/// Marks a TOML or YAML key, or a TOML table header, and returns where the value starts.
fn key(language: Language, line: &str, tokens: &mut Vec<(Range<usize>, Token)>) -> usize {
    let start = line.len() - line.trim_start().len();
    let trimmed = &line[start..];

    if language == Language::Toml && trimmed.starts_with('[') {
        let end = trimmed.find(']').map_or(line.len(), |end| start + end + 1);

        tokens.push((start..end, Token::Keyword));
        return end;
    }

    let start = if language == Language::Yaml && trimmed.starts_with("- ") {
        start + 2 + trimmed[2..].len() - trimmed[2..].trim_start().len()
    } else {
        start
    };

    let separator = if language == Language::Toml { '=' } else { ':' };
    let Some(end) = line[start..].find(separator).map(|end| start + end) else {
        return start;
    };

    let name = line[start..end].trim_end();
    let valid = !name.is_empty()
        && name.chars().all(|character| {
            character.is_alphanumeric() || matches!(character, '_' | '-' | '.' | '"' | '\'' | ' ')
        });

    if valid {
        tokens.push((start..start + name.len(), Token::Keyword));
        end
    } else {
        start
    }
}

/// Where the string closed by QUOTE ends in TEXT, past the quote, skipping escapes.
fn closing(text: &str, quote: &str) -> Option<usize> {
    let mut escaped = false;

    for (offset, character) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if character == '\\' && quote != "'''" {
            escaped = true;
        } else if text[offset..].starts_with(quote) {
            return Some(offset + quote.len());
        }
    }

    None
}

/// The length of a Rust character literal at the start of TEXT, unlike a lifetime.
fn character(text: &str) -> Option<usize> {
    let mut characters = text.char_indices().skip(1);
    let (_, first) = characters.next()?;

    if first == '\\' {
        return text[2..].find('\'').map(|end| end + 3).filter(|end| *end <= 12);
    }

    let (offset, quote) = characters.next()?;

    (quote == '\'').then_some(offset + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(language: &str, source: &str) -> Vec<Vec<(String, Token)>> {
        let language = Language::named(language).unwrap();
        let mut state = State::default();

        source
            .lines()
            .map(|text| {
                line(language, &mut state, text)
                    .into_iter()
                    .map(|(range, token)| (text[range].to_owned(), token))
                    .collect()
            })
            .collect()
    }

    fn of(tokens: &[Vec<(String, Token)>], kind: Token) -> Vec<&str> {
        tokens.iter().flatten().filter(|(_, token)| *token == kind).map(|(text, _)| text.as_str()).collect()
    }

    #[test]
    fn highlights_rust_keywords_strings_comments_numbers_and_lifetimes() {
        let found = tokens(
            "rust",
            "pub fn main<'a>() -> u8 { // note\n    let c = 'x'; let s = \"a \\\" b\";\n    \
             /* one\n two */ 0x1F + 2_000 + x1 }",
        );

        assert_eq!(of(&found, Token::Keyword), ["pub", "fn", "let", "let"]);
        assert_eq!(of(&found, Token::Comment), ["// note", "/* one", " two */"]);
        assert_eq!(of(&found, Token::Text), ["'x'", "\"a \\\" b\""]);
        assert_eq!(of(&found, Token::Number), ["0x1F", "2_000"]);
    }

    #[test]
    fn highlights_each_supported_language() {
        let python = tokens("py", "def f(x):  # doc\n    return \"\"\"multi\nline\"\"\" if x else 3");

        assert_eq!(of(&python, Token::Keyword), ["def", "return", "if", "else"]);
        assert_eq!(of(&python, Token::Text), ["\"\"\"multi", "line\"\"\""]);
        assert_eq!(of(&python, Token::Number), ["3"]);
        assert_eq!(of(&python, Token::Comment), ["# doc"]);

        let script = tokens("ts", "const total = `a ${b}`; // sum\nexport function f() {}");

        assert_eq!(of(&script, Token::Keyword), ["const", "export", "function"]);
        assert_eq!(of(&script, Token::Text), ["`a ${b}`"]);

        let shell = tokens("bash", "if [ \"$#\" -gt 1 ]; then echo 'hi' # note\nfi");

        assert_eq!(of(&shell, Token::Keyword), ["if", "then", "fi"]);
        assert_eq!(of(&shell, Token::Text), ["\"$#\"", "'hi'"]);
        assert_eq!(of(&shell, Token::Comment), ["# note"]);
        assert_eq!(of(&shell, Token::Number), ["1"]);

        let json = tokens("json", "{\"a\": [1.5, true, null]}");

        assert_eq!(of(&json, Token::Text), ["\"a\""]);
        assert_eq!(of(&json, Token::Number), ["1.5"]);
        assert_eq!(of(&json, Token::Keyword), ["true", "null"]);

        let toml = tokens("toml", "[package]\nname = \"krabb\" # crate\nlevel = 3");

        assert_eq!(of(&toml, Token::Keyword), ["[package]", "name", "level"]);
        assert_eq!(of(&toml, Token::Text), ["\"krabb\""]);
        assert_eq!(of(&toml, Token::Comment), ["# crate"]);

        let yaml = tokens("yml", "jobs:\n  - name: build # step\n    on: true");

        assert_eq!(of(&yaml, Token::Keyword), ["jobs", "name", "on", "true"]);
        assert_eq!(of(&yaml, Token::Comment), ["# step"]);

        let go = tokens("go", "func main() { fmt.Println(\"hi\", 42) }");

        assert_eq!(of(&go, Token::Keyword), ["func"]);
        assert_eq!(of(&go, Token::Number), ["42"]);
    }

    #[test]
    fn colors_unified_diff_lines_and_detects_unlabeled_diffs() {
        let found = tokens("diff", "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n same");
        let kinds = found.iter().map(|line| line[0].1).collect::<Vec<_>>();

        assert_eq!(
            kinds,
            [Token::Header, Token::Header, Token::Hunk, Token::Removed, Token::Added, Token::Plain]
        );

        assert_eq!(Language::detect("", &["--- a", "+++ b", "@@ -1 +1 @@"]), Some(Language::Diff));
        assert_eq!(Language::detect("code", &["- item", "+ item"]), None);
        assert_eq!(Language::detect("elixir", &["defmodule"]), None);
    }

    #[test]
    fn unterminated_strings_and_unicode_stay_in_bounds() {
        let found = tokens("rust", "let s = \"open 界\nlet é = 'é';");

        assert_eq!(of(&found, Token::Text), ["\"open 界", "'é'"]);

        for language in ["rust", "py", "ts", "sh", "json", "toml", "yaml", "go", "diff"] {
            for source in ["\"", "'", "/*", "`", "#", "'\\", "界'", "0", "a:", "[", "- "] {
                let _ = tokens(language, source);
            }
        }
    }
}
