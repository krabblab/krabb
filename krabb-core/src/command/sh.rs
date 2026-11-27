use super::program::{deletes, reads, shared, unknown, writes};
use super::syntax::{Token, Word, long, operands};
use super::{Kind, Verdict};

const OPEN: &[&str] = &[
    "ls",
    "cat",
    "head",
    "tail",
    "wc",
    "stat",
    "du",
    "df",
    "od",
    "hexdump",
    "pwd",
    "echo",
    "which",
    "uname",
    "printenv",
    "grep",
    "egrep",
    "fgrep",
    "cut",
    "tr",
    "column",
    "diff",
    "cmp",
    "md5sum",
    "sha1sum",
    "sha224sum",
    "sha256sum",
    "sha384sum",
    "sha512sum",
    "b2sum",
    "cksum",
    "basename",
    "dirname",
    "true",
    "false",
];

const DELETES: &[&str] = &["rm", "rmdir", "unlink", "shred"];

const WRITES: &[&str] = &[
    "tee", "mv", "cp", "touch", "mkdir", "chmod", "chown", "chgrp", "ln", "dd", "truncate",
    "install", "mkfifo", "mknod", "patch",
];

const FIND_RUNS: &[&str] = &["-exec", "-execdir", "-ok", "-okdir"];
const FIND_WRITES: &[&str] = &["-fprint", "-fprint0", "-fprintf", "-fls"];

const XXD_VALUES: &[&str] =
    &["-c", "-cols", "-g", "-groupsize", "-l", "-len", "-s", "-seek", "-o", "-offset", "-n"];

const DATE_LONG: &[&str] = &[
    "date",
    "reference",
    "file",
    "utc",
    "universal",
    "iso-8601",
    "rfc-email",
    "rfc-3339",
    "debug",
    "resolution",
];

const XARGS_VALUES: &[&str] = &["-n", "-L", "-I", "-d", "-P", "-s", "-E", "-a"];
const XARGS_FLAGS: &[&str] = &["-0", "-r", "-t", "-x", "--null", "--no-run-if-empty", "--verbose"];

const XARGS_LONG: &[&str] = &[
    "--max-args=",
    "--max-lines=",
    "--replace=",
    "--delimiter=",
    "--max-procs=",
    "--max-chars=",
    "--eof=",
    "--arg-file=",
];

const BREAKS: &[char] = &[' ', '\t', '\n', '|', '&', ';', '(', ')', '<', '>'];

pub(super) fn open(name: &str) -> bool {
    OPEN.contains(&name)
}

pub(super) fn program(name: &str, args: &[&str]) -> Verdict {
    if open(name) {
        return reads(name);
    }

    match name {
        "file" => file(args),
        "xxd" => xxd(args),
        "rg" => ripgrep(args),
        "find" => find(args),
        "sort" => sort(args),
        "uniq" => uniq(args),
        "printf" => printf(args),
        "date" => date(args),
        "env" => env(args),
        "xargs" => xargs(args),
        _ if DELETES.contains(&name) => deletes(name),
        _ if WRITES.contains(&name) => writes(name),
        _ => shared(name, args).unwrap_or_else(|| unknown(name)),
    }
}

fn file(args: &[&str]) -> Verdict {
    let compiles = args.iter().any(|arg| {
        long(arg, "compile")
            || (!arg.starts_with("--") && arg.starts_with('-') && arg.contains('C'))
    });

    if compiles {
        return Verdict::new(Kind::Writes, "`file -C` writes a magic file");
    }

    reads("file")
}

fn xxd(args: &[&str]) -> Verdict {
    if operands(args, XXD_VALUES) > 1 {
        return Verdict::new(Kind::Writes, "`xxd` writes its output file");
    }

    reads("xxd")
}

fn ripgrep(args: &[&str]) -> Verdict {
    let runs = args.iter().any(|arg| {
        let name = arg.split_once('=').map_or(*arg, |(name, _)| name);

        matches!(name, "--pre" | "--hostname-bin")
    });

    if runs {
        return Verdict::new(Kind::Unknown, "`rg --pre` runs another program");
    }

    reads("rg")
}

fn find(args: &[&str]) -> Verdict {
    let mut verdict = reads("find");

    for arg in args {
        let next = if FIND_RUNS.contains(arg) {
            Verdict::new(Kind::Unknown, format!("`find {arg}` runs other commands"))
        } else if *arg == "-delete" {
            Verdict::new(Kind::Deletes, "`find -delete` deletes files")
        } else if FIND_WRITES.contains(arg) {
            Verdict::new(Kind::Writes, format!("`find {arg}` writes a file"))
        } else {
            continue;
        };

        verdict = verdict.max(next);
    }

    verdict
}

fn sort(args: &[&str]) -> Verdict {
    for arg in args.iter().take_while(|arg| **arg != "--") {
        if long(arg, "output") {
            return Verdict::new(Kind::Writes, "`sort --output` writes a file");
        }

        if long(arg, "compress-program") {
            return Verdict::new(Kind::Unknown, "`sort --compress-program` runs another program");
        }

        if arg.starts_with("--") {
            continue;
        }

        let Some(flags) = arg.strip_prefix('-') else {
            continue;
        };

        for flag in flags.chars() {
            if flag == 'o' {
                return Verdict::new(Kind::Writes, "`sort -o` writes a file");
            }

            if "ktST".contains(flag) {
                break;
            }
        }
    }

    reads("sort")
}

fn uniq(args: &[&str]) -> Verdict {
    if operands(args, &["-f", "-s", "-w"]) > 1 {
        return Verdict::new(Kind::Writes, "`uniq` writes its output file");
    }

    reads("uniq")
}

fn printf(args: &[&str]) -> Verdict {
    if args.first().is_some_and(|arg| arg.starts_with("-v")) {
        return Verdict::new(Kind::Unknown, "`printf -v` sets a variable");
    }

    reads("printf")
}

fn date(args: &[&str]) -> Verdict {
    let sets = Verdict::new(Kind::Writes, "`date` sets the clock");
    let mut index = 0;

    while let Some(&arg) = args.get(index) {
        index += 1;

        if arg.starts_with('+') {
            continue;
        }

        if arg.starts_with("-s") || long(arg, "set") {
            return sets;
        }

        if matches!(arg, "-d" | "-r" | "-f" | "--date" | "--reference" | "--file") {
            index += 1;
            continue;
        }

        let known = if arg.starts_with("--") {
            DATE_LONG.iter().any(|option| long(arg, option))
        } else if arg.starts_with('-') {
            matches!(arg, "-u" | "-R")
                || ["-I", "-d", "-r", "-f"].iter().any(|o| arg.starts_with(o))
        } else {
            return sets;
        };

        if !known {
            return unknown("date");
        }
    }

    reads("date")
}

fn env(args: &[&str]) -> Verdict {
    if args.is_empty() {
        return reads("env");
    }

    Verdict::new(Kind::Unknown, "`env` runs a command with a changed environment")
}

fn xargs(args: &[&str]) -> Verdict {
    let mut index = 0;

    while let Some(&arg) = args.get(index) {
        if arg == "--" {
            index += 1;
            break;
        }

        if !arg.starts_with('-') {
            break;
        }

        index += 1;

        if XARGS_VALUES.contains(&arg) {
            index += 1;
        } else if !(XARGS_FLAGS.contains(&arg)
            || XARGS_LONG.iter().any(|option| arg.starts_with(option))
            || XARGS_VALUES.iter().any(|option| arg.starts_with(option)))
        {
            return Verdict::new(Kind::Unknown, format!("`xargs {arg}` is not known to be safe"));
        }
    }

    let Some((&name, rest)) = args.get(index..).and_then(<[&str]>::split_first) else {
        return reads("xargs");
    };

    if name.contains('/') {
        return Verdict::new(Kind::Unknown, "`xargs` runs a program by path");
    }

    let verdict = program(name, rest);

    if open(name) {
        return verdict;
    }

    verdict.cap(&format!("`xargs` adds arguments to `{name}`"))
}

pub(super) fn lex(input: &str) -> Result<Vec<Token>, &'static str> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;

    while let Some(&c) = chars.get(index) {
        let next = chars.get(index + 1).copied();

        match c {
            ' ' | '\t' => index += 1,

            '\n' => {
                tokens.push(Token::Separator);
                index += 1;
            }

            '|' => {
                tokens.push(Token::Separator);
                index += 1 + usize::from(matches!(next, Some('|' | '&')));
            }

            '&' => match next {
                Some('&') => {
                    tokens.push(Token::Separator);
                    index += 2;
                }

                Some('>') => {
                    tokens.push(Token::Output);
                    index += 2 + usize::from(chars.get(index + 2) == Some(&'>'));
                }

                _ => return Err("the command runs in the background"),
            },

            ';' if next == Some(';') => return Err("the command has a case terminator"),

            ';' => {
                tokens.push(Token::Separator);
                index += 1;
            }

            '(' | ')' => return Err("the command opens a subshell"),
            '#' => return Err("the command has a comment"),

            '<' => match next {
                Some('<') => return Err("the command has a heredoc"),
                Some('(') => return Err("the command uses process substitution"),

                Some('>') => {
                    tokens.push(Token::Output);
                    index += 2;
                }

                Some('&') => {
                    index = duplicate(&chars, index + 2)
                        .ok_or("the command has an unusual redirection")?;
                }

                _ => {
                    tokens.push(Token::Input);
                    index += 1;
                }
            },

            '>' => match next {
                Some('(') => return Err("the command uses process substitution"),

                Some('&') => match duplicate(&chars, index + 2) {
                    Some(end) => index = end,

                    None => {
                        tokens.push(Token::Output);
                        index += 2;
                    }
                },

                Some('>' | '|') => {
                    tokens.push(Token::Output);
                    index += 2;
                }

                _ => {
                    tokens.push(Token::Output);
                    index += 1;
                }
            },

            _ => {
                let (word, end) = word(&chars, index)?;
                let number = !word.quoted && word.text.chars().all(|c| c.is_ascii_digit());

                let empty = !word.quoted && word.text.is_empty();

                if !empty && !(number && matches!(chars.get(end), Some('<' | '>'))) {
                    tokens.push(Token::Word(word));
                }

                index = end;
            }
        }
    }

    Ok(tokens)
}

fn duplicate(chars: &[char], start: usize) -> Option<usize> {
    let mut end = start;

    while chars.get(end).is_some_and(char::is_ascii_digit) {
        end += 1;
    }

    if end == start && chars.get(end) == Some(&'-') {
        end += 1;
    }

    let bounded = chars.get(end).is_none_or(|c| BREAKS.contains(c));

    (end > start && bounded).then_some(end)
}

fn word(chars: &[char], mut index: usize) -> Result<(Word, usize), &'static str> {
    let mut word = Word::default();
    let mut name = true;
    let mut brace = false;

    while let Some(&c) = chars.get(index) {
        if BREAKS.contains(&c) {
            break;
        }

        let ident = c == '_' || c.is_ascii_alphanumeric();

        match c {
            '\\' => match chars.get(index + 1) {
                Some('\n') => index += 2,

                Some(&escaped) => {
                    word.text.push(escaped);
                    word.quoted = true;
                    index += 2;
                }

                None => return Err("the command ends with a backslash"),
            },

            '\'' => {
                let close = chars[index + 1..]
                    .iter()
                    .position(|c| *c == '\'')
                    .ok_or("a quote is not closed")?;

                word.text.extend(&chars[index + 1..index + 1 + close]);
                word.quoted = true;
                index += close + 2;
            }

            '"' => index = double(chars, index + 1, &mut word)?,
            '$' => index = dollar(chars, index, &mut word)?,
            '`' => return Err("the command uses command substitution"),

            '{' => {
                brace = true;
                word.text.push(c);
                index += 1;
            }

            '*' | '?' | '[' => {
                word.glob = true;
                word.text.push(c);
                index += 1;
            }

            '=' if name && !word.text.is_empty() && !word.assignment => {
                word.assignment = true;
                word.text.push(c);
                index += 1;
            }

            '+' if name && chars.get(index + 1) == Some(&'=') => {
                word.text.push(c);
                index += 1;
                continue;
            }

            _ => {
                word.text.push(c);
                index += 1;

                if ident {
                    continue;
                }
            }
        }

        name = false;
    }

    if word.text.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        word.assignment = false;
    }

    if brace && (word.text.contains(',') || word.text.contains("..")) {
        word.glob = true;
    }

    Ok((word, index))
}

fn double(chars: &[char], mut index: usize, word: &mut Word) -> Result<usize, &'static str> {
    word.quoted = true;

    loop {
        match chars.get(index) {
            None => return Err("a quote is not closed"),
            Some('"') => return Ok(index + 1),
            Some('`') => return Err("the command uses command substitution"),
            Some('$') => index = dollar(chars, index, word)?,

            Some('\\') => match chars.get(index + 1) {
                Some('\n') => index += 2,

                Some(&escaped @ ('$' | '`' | '"' | '\\')) => {
                    word.text.push(escaped);
                    index += 2;
                }

                _ => {
                    word.text.push('\\');
                    index += 1;
                }
            },

            Some(&c) => {
                word.text.push(c);
                index += 1;
            }
        }
    }
}

fn dollar(chars: &[char], index: usize, word: &mut Word) -> Result<usize, &'static str> {
    match chars.get(index + 1) {
        Some('(') => Err("the command uses command substitution"),
        Some('{') => Err("the command uses parameter expansion"),
        Some('[') => Err("the command uses arithmetic expansion"),
        Some('\'' | '"') => Err("the command uses special quoting"),

        Some(&c) if c == '_' || c.is_ascii_alphabetic() => {
            let length = chars[index + 1..]
                .iter()
                .take_while(|c| **c == '_' || c.is_ascii_alphanumeric())
                .count();

            word.dynamic = true;
            word.text.extend(&chars[index..index + 1 + length]);
            Ok(index + 1 + length)
        }

        Some(&c) if c.is_ascii_digit() || "?#@*!$-".contains(c) => {
            word.dynamic = true;
            word.text.push('$');
            word.text.push(c);
            Ok(index + 2)
        }

        _ => {
            word.text.push('$');
            Ok(index + 1)
        }
    }
}
