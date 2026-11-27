use super::program::{deletes, elevates, installs, reads, shared, unknown, writes};
use super::syntax::{Token, Word};
use super::{Kind, Verdict};

const ALIASES: &[(&str, &str)] = &[
    ("ls", "get-childitem"),
    ("dir", "get-childitem"),
    ("gci", "get-childitem"),
    ("cat", "get-content"),
    ("type", "get-content"),
    ("gc", "get-content"),
    ("sls", "select-string"),
    ("pwd", "get-location"),
    ("gl", "get-location"),
    ("gi", "get-item"),
    ("echo", "write-output"),
    ("write", "write-output"),
    ("measure", "measure-object"),
    ("select", "select-object"),
    ("sort", "sort-object"),
    ("rm", "remove-item"),
    ("del", "remove-item"),
    ("erase", "remove-item"),
    ("rd", "remove-item"),
    ("rmdir", "remove-item"),
    ("ri", "remove-item"),
    ("cp", "copy-item"),
    ("copy", "copy-item"),
    ("cpi", "copy-item"),
    ("mv", "move-item"),
    ("move", "move-item"),
    ("mi", "move-item"),
    ("ren", "rename-item"),
    ("rni", "rename-item"),
    ("ni", "new-item"),
    ("mkdir", "new-item"),
    ("md", "new-item"),
    ("sc", "set-content"),
    ("ac", "add-content"),
    ("clc", "clear-content"),
    ("tee", "tee-object"),
    ("saps", "start-process"),
    ("start", "start-process"),
];

const READS: &[&str] = &[
    "get-childitem",
    "get-content",
    "select-string",
    "test-path",
    "get-location",
    "get-item",
    "get-date",
    "get-filehash",
    "resolve-path",
    "write-output",
    "measure-object",
    "select-object",
    "sort-object",
    "findstr",
];

const VARIABLES: &[&str] =
    &["outvariable", "errorvariable", "warningvariable", "informationvariable", "pipelinevariable"];

const SHORT_VARIABLES: &[&str] = &["ov", "ev", "wv", "iv", "pv"];

const WRITE_VERBS: &[&str] =
    &["set-", "new-", "add-", "copy-", "move-", "rename-", "clear-", "export-"];

const WRITE_CMDLETS: &[&str] = &["out-file", "tee-object"];

const DISGUISES: &[char] = &[
    '\u{2013}', '\u{2014}', '\u{2015}', '\u{2018}', '\u{2019}', '\u{201a}', '\u{201b}', '\u{201c}',
    '\u{201d}', '\u{201e}',
];

const BREAKS: &[char] = &[' ', '\t', '\n', ';', '|', '&', '<', '>'];
const EXPRESSIONS: &[char] = &['(', ')', '{', '}', '[', ']'];

pub(super) fn program(name: &str, args: &[&str]) -> Verdict {
    let name = ALIASES.iter().find(|(alias, _)| *alias == name).map_or(name, |(_, cmdlet)| cmdlet);

    if READS.contains(&name) {
        if args.iter().any(|arg| sets_variable(arg)) {
            return Verdict::new(Kind::Unknown, "the command sets a variable");
        }

        return reads(name);
    }

    if name == "start-process" {
        if args.iter().any(|arg| arg.eq_ignore_ascii_case("runas")) {
            return elevates(name);
        }

        return unknown(name);
    }

    if name.starts_with("remove-") {
        return deletes(name);
    }

    if WRITE_CMDLETS.contains(&name) || WRITE_VERBS.iter().any(|verb| name.starts_with(verb)) {
        return writes(name);
    }

    if name.starts_with("install-") || name.starts_with("uninstall-") {
        return installs(name);
    }

    shared(name, args).unwrap_or_else(|| unknown(name))
}

fn sets_variable(arg: &str) -> bool {
    let Some(parameter) = arg.strip_prefix('-') else {
        return false;
    };

    let parameter = parameter.split_once(':').map_or(parameter, |(name, _)| name);
    let parameter = parameter.to_ascii_lowercase();

    !parameter.is_empty()
        && (SHORT_VARIABLES.contains(&parameter.as_str())
            || VARIABLES.iter().any(|long| long.starts_with(&parameter)))
}

pub(super) fn lex(input: &str) -> Result<Vec<Token>, &'static str> {
    if input.contains(DISGUISES) {
        return Err("the command has a typographic quote or dash");
    }

    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;

    while let Some(&c) = chars.get(index) {
        let next = chars.get(index + 1).copied();

        match c {
            ' ' | '\t' => index += 1,

            '\n' | ';' => {
                tokens.push(Token::Separator);
                index += 1;
            }

            '|' => {
                tokens.push(Token::Separator);
                index += 1 + usize::from(next == Some('|'));
            }

            '&' if next == Some('&') => {
                tokens.push(Token::Separator);
                index += 2;
            }

            '&' => return Err("the command uses the call operator"),
            '<' => return Err("the command uses a reserved operator"),
            '#' => return Err("the command has a comment"),
            '@' => return Err("the command has an expression"),

            '>' if next == Some('&') => {
                let digits = chars[index + 2..].iter().take_while(|c| c.is_ascii_digit()).count();

                if digits == 0 {
                    return Err("the command has an unusual redirection");
                }

                index += 2 + digits;
            }

            '>' => {
                tokens.push(Token::Output);
                index += 1 + usize::from(next == Some('>'));
            }

            _ => {
                let (word, end) = word(&chars, index)?;
                let stream = word.text == "*" || word.text.chars().all(|c| c.is_ascii_digit());

                if !(stream && !word.quoted && chars.get(end) == Some(&'>')) {
                    tokens.push(Token::Word(word));
                }

                index = end;
            }
        }
    }

    Ok(tokens)
}

fn word(chars: &[char], mut index: usize) -> Result<(Word, usize), &'static str> {
    let mut word = Word::default();

    while let Some(&c) = chars.get(index) {
        if BREAKS.contains(&c) {
            break;
        }

        if EXPRESSIONS.contains(&c) {
            return Err("the command has an expression");
        }

        match c {
            '`' => return Err("the command has an escape character"),
            '\'' => index = single(chars, index + 1, &mut word)?,
            '"' => index = double(chars, index + 1, &mut word)?,

            '$' => {
                if matches!(chars.get(index + 1), Some('(' | '{')) {
                    return Err("the command has an expression");
                }

                word.dynamic = true;
                word.text.push(c);
                index += 1;
            }

            _ => {
                word.text.push(c);
                index += 1;
            }
        }
    }

    if word.text == "--%" {
        return Err("the command stops parsing");
    }

    Ok((word, index))
}

fn single(chars: &[char], mut index: usize, word: &mut Word) -> Result<usize, &'static str> {
    word.quoted = true;

    loop {
        match (chars.get(index), chars.get(index + 1)) {
            (None, _) => return Err("a quote is not closed"),

            (Some('\''), Some('\'')) => {
                word.text.push('\'');
                index += 2;
            }

            (Some('\''), _) => return Ok(index + 1),

            (Some(&c), _) => {
                word.text.push(c);
                index += 1;
            }
        }
    }
}

fn double(chars: &[char], mut index: usize, word: &mut Word) -> Result<usize, &'static str> {
    word.quoted = true;

    loop {
        match (chars.get(index), chars.get(index + 1)) {
            (None, _) => return Err("a quote is not closed"),
            (Some('$' | '`'), _) => return Err("a double-quoted string expands"),

            (Some('"'), Some('"')) => {
                word.text.push('"');
                index += 2;
            }

            (Some('"'), _) => return Ok(index + 1),

            (Some(&c), _) => {
                word.text.push(c);
                index += 1;
            }
        }
    }
}
