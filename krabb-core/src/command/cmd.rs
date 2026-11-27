use super::Verdict;
use super::program::{deletes, reads, shared, unknown, writes};
use super::syntax::{Token, Word};

const READS: &[&str] = &["dir", "type", "findstr", "where", "ver", "echo"];
const DELETES: &[&str] = &["del", "erase", "rd", "rmdir"];

const WRITES: &[&str] = &[
    "copy", "move", "xcopy", "robocopy", "ren", "rename", "md", "mkdir", "mklink", "attrib",
    "icacls", "takeown",
];

const BREAKS: &[char] = &[' ', '\t', '\n', '&', '|', '<', '>', '(', ')'];

pub(super) fn program(name: &str, args: &[&str]) -> Verdict {
    if READS.contains(&name) {
        return reads(name);
    }

    if DELETES.contains(&name) {
        return deletes(name);
    }

    if WRITES.contains(&name) {
        return writes(name);
    }

    shared(name, args).unwrap_or_else(|| unknown(name))
}

pub(super) fn lex(input: &str) -> Result<Vec<Token>, &'static str> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;

    while let Some(&c) = chars.get(index) {
        let next = chars.get(index + 1).copied();

        match c {
            ' ' | '\t' => index += 1,
            '\n' => return Err("the command has more than one line"),
            '(' | ')' => return Err("the command has a group"),

            '&' | '|' => {
                tokens.push(Token::Separator);
                index += 1 + usize::from(next == Some(c));
            }

            '<' | '>' if next == Some('&') => {
                let digits = chars[index + 2..].iter().take_while(|c| c.is_ascii_digit()).count();

                let bounded = chars.get(index + 2 + digits).is_none_or(|c| BREAKS.contains(c));

                if digits == 0 || !bounded {
                    return Err("the command has an unusual redirection");
                }

                index += 2 + digits;
            }

            '<' => {
                tokens.push(Token::Input);
                index += 1;
            }

            '>' => {
                tokens.push(Token::Output);
                index += 1 + usize::from(next == Some('>'));
            }

            _ => {
                let (word, end) = word(&chars, index)?;
                let number = !word.quoted && word.text.chars().all(|c| c.is_ascii_digit());

                if !(number && matches!(chars.get(end), Some('<' | '>'))) {
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
    let mut quoted = false;

    while let Some(&c) = chars.get(index) {
        if !quoted && BREAKS.contains(&c) {
            break;
        }

        match c {
            '"' => {
                quoted = !quoted;
                word.quoted = true;
            }

            '^' => return Err("the command has an escape character"),
            '%' => return Err("the command expands a variable"),
            '!' => return Err("the command uses delayed expansion"),
            '\n' => return Err("the command has more than one line"),
            _ => word.text.push(c),
        }

        index += 1;
    }

    if quoted {
        return Err("a quote is not closed");
    }

    Ok((word, index))
}
