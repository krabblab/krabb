use super::{Lexed, Token, Word};
use crate::command::Hazard;

const BREAKS: &[char] = &[' ', '\t', '\n', '|', '&', ';', '(', ')', '<', '>', '`'];

pub(in crate::command) fn lex(input: &str) -> Lexed {
    let chars: Vec<char> = input.chars().collect();
    let mut lexed = Lexed::default();
    let mut heredoc = false;
    let mut index = 0;

    while let Some(&c) = chars.get(index) {
        let next = chars.get(index + 1).copied();

        match c {
            ' ' | '\t' => index += 1,
            '\n' if heredoc => break,

            '\n' | ';' if !(c == ';' && next == Some(';')) => {
                lexed.push(Token::Separator);
                index += 1;
            }

            ';' => {
                lexed.hazard(Hazard::CaseTerminator);
                lexed.push(Token::Boundary);
                index += 2;
            }

            '|' => {
                lexed.push(Token::Separator);
                index += 1 + usize::from(matches!(next, Some('|' | '&')));
            }

            '&' if next == Some('&') => {
                lexed.push(Token::Separator);
                index += 2;
            }

            '&' => {
                lexed.hazard(Hazard::Background);
                lexed.push(Token::Boundary);
                index += 1;
            }

            '(' | ')' => {
                lexed.hazard(Hazard::Subshell);
                lexed.push(Token::Boundary);
                index += 1;
            }

            '`' => {
                lexed.hazard(Hazard::Substitution);
                lexed.push(Token::Boundary);
                index += 1;
            }

            '$' if next == Some('(') => {
                lexed.hazard(Hazard::Substitution);
                lexed.push(Token::Boundary);
                index += 2;
            }

            '#' => {
                lexed.hazard(Hazard::Comment);
                index += chars[index..].iter().take_while(|c| **c != '\n').count();
            }

            '<' => index = input_redirection(&chars, index, &mut lexed, &mut heredoc),
            '>' => index = output_redirection(&chars, index, &mut lexed),

            _ => {
                let (word, end) = word(&chars, index, &mut lexed);

                lexed.word(word, chars.get(end));
                index = end;
            }
        }
    }

    lexed
}

fn input_redirection(chars: &[char], index: usize, lexed: &mut Lexed, heredoc: &mut bool) -> usize {
    match chars.get(index + 1) {
        Some('<') => {
            lexed.hazard(Hazard::Heredoc);
            lexed.push(Token::Input);
            *heredoc = true;
            index + 2 + usize::from(matches!(chars.get(index + 2), Some('<' | '-')))
        }

        Some('(') => {
            lexed.hazard(Hazard::ProcessSubstitution);
            lexed.push(Token::Boundary);
            index + 2
        }

        Some('>') => {
            lexed.push(Token::Output);
            index + 2
        }

        Some('&') => duplicate(chars, index + 2).unwrap_or_else(|| {
            lexed.hazard(Hazard::Redirection);
            lexed.push(Token::Input);
            index + 2
        }),

        _ => {
            lexed.push(Token::Input);
            index + 1
        }
    }
}

fn output_redirection(chars: &[char], index: usize, lexed: &mut Lexed) -> usize {
    match chars.get(index + 1) {
        Some('(') => {
            lexed.hazard(Hazard::ProcessSubstitution);
            lexed.push(Token::Boundary);
            index + 2
        }

        Some('&') => duplicate(chars, index + 2).unwrap_or_else(|| {
            lexed.push(Token::Output);
            index + 2
        }),

        Some('>' | '|') => {
            lexed.push(Token::Output);
            index + 2
        }

        _ => {
            lexed.push(Token::Output);
            index + 1
        }
    }
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

fn word(chars: &[char], mut index: usize, lexed: &mut Lexed) -> (Word, usize) {
    let mut word = Word::default();
    let mut name = true;
    let mut brace = false;

    while let Some(&c) = chars.get(index) {
        if BREAKS.contains(&c) || c == '$' && chars.get(index + 1) == Some(&'(') {
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

                None => {
                    lexed.hazard(Hazard::Backslash);
                    index += 1;
                }
            },

            '\'' => index = single(chars, index + 1, &mut word, lexed),
            '"' => index = double(chars, index + 1, &mut word, lexed),
            '$' => index = dollar(chars, index, &mut word, lexed),

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

    (word, index)
}

fn single(chars: &[char], start: usize, word: &mut Word, lexed: &mut Lexed) -> usize {
    word.quoted = true;

    let Some(close) = chars[start..].iter().position(|c| *c == '\'') else {
        lexed.hazard(Hazard::Unclosed);
        word.text.extend(&chars[start..]);
        return chars.len();
    };

    word.text.extend(&chars[start..start + close]);
    start + close + 1
}

fn double(chars: &[char], mut index: usize, word: &mut Word, lexed: &mut Lexed) -> usize {
    word.quoted = true;

    loop {
        match chars.get(index) {
            None => {
                lexed.hazard(Hazard::Unclosed);
                return index;
            }

            Some('"') => return index + 1,

            Some('`') => {
                lexed.hazard(Hazard::Substitution);
                word.dynamic = true;
                word.text.push('`');
                index += 1;
            }

            Some('$') => index = dollar(chars, index, word, lexed),

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

fn dollar(chars: &[char], index: usize, word: &mut Word, lexed: &mut Lexed) -> usize {
    match chars.get(index + 1) {
        Some('(') => {
            lexed.hazard(Hazard::Substitution);
            word.dynamic = true;
            word.text.push('$');
            index + 1
        }

        Some(&open @ ('{' | '[')) => {
            let close = if open == '{' { '}' } else { ']' };

            let length = chars[index + 1..].iter().take_while(|c| **c != close).count();
            let end = (index + 2 + length).min(chars.len());

            lexed.hazard(Hazard::Expansion);
            word.dynamic = true;
            word.text.extend(&chars[index..end]);
            end
        }

        Some('\'' | '"') => {
            lexed.hazard(Hazard::SpecialQuote);
            word.dynamic = true;
            index + 1
        }

        Some(&c) if c == '_' || c.is_ascii_alphabetic() => {
            let length = chars[index + 1..]
                .iter()
                .take_while(|c| **c == '_' || c.is_ascii_alphanumeric())
                .count();

            word.dynamic = true;
            word.text.extend(&chars[index..index + 1 + length]);
            index + 1 + length
        }

        Some(&c) if c.is_ascii_digit() || "?#@*!$-".contains(c) => {
            word.dynamic = true;
            word.text.push('$');
            word.text.push(c);
            index + 2
        }

        _ => {
            word.text.push('$');
            index + 1
        }
    }
}
