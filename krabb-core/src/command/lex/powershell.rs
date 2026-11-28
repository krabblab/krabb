use super::{Lexed, Token, Word};
use crate::command::Hazard;

const TYPOGRAPHIC: &[char] = &[
    '\u{2013}', '\u{2014}', '\u{2015}', '\u{2018}', '\u{2019}', '\u{201a}', '\u{201b}', '\u{201c}',
    '\u{201d}', '\u{201e}',
];

const BREAKS: &[char] = &[' ', '\t', '\n', ';', '|', '&', '<', '>', '(', ')', '{', '}'];

pub(in crate::command) fn lex(input: &str) -> Lexed {
    let chars: Vec<char> = input.chars().collect();
    let mut lexed = Lexed::default();
    let mut index = 0;

    if input.contains(TYPOGRAPHIC) {
        lexed.hazard(Hazard::Typographic);
    }

    while let Some(&c) = chars.get(index) {
        let next = chars.get(index + 1).copied();

        match c {
            ' ' | '\t' => index += 1,

            '\n' | ';' => {
                lexed.push(Token::Separator);
                index += 1;
            }

            '|' => {
                lexed.push(Token::Separator);
                index += 1 + usize::from(next == Some('|'));
            }

            '&' if next == Some('&') => {
                lexed.push(Token::Separator);
                index += 2;
            }

            '&' => {
                lexed.hazard(Hazard::Call);
                lexed.push(Token::Boundary);
                index += 1;
            }

            '<' => {
                lexed.hazard(Hazard::Reserved);
                index += 1;
            }

            '#' => {
                lexed.hazard(Hazard::Comment);
                index += chars[index..].iter().take_while(|c| **c != '\n').count();
            }

            '(' | ')' | '{' | '}' => {
                lexed.hazard(Hazard::Expression);
                lexed.push(Token::Boundary);
                index += 1;
            }

            '$' if next == Some('(') => {
                lexed.hazard(Hazard::Expression);
                lexed.push(Token::Boundary);
                index += 2;
            }

            '>' if next == Some('&') => {
                let digits = chars[index + 2..].iter().take_while(|c| c.is_ascii_digit()).count();

                if digits == 0 {
                    lexed.hazard(Hazard::Redirection);
                    lexed.push(Token::Output);
                }

                index += 2 + digits;
            }

            '>' => {
                lexed.push(Token::Output);
                index += 1 + usize::from(next == Some('>'));
            }

            _ => {
                let (word, end) = word(&chars, index, &mut lexed);

                lexed.word(word, chars.get(end));
                index = end;
            }
        }
    }

    lexed
}

fn word(chars: &[char], mut index: usize, lexed: &mut Lexed) -> (Word, usize) {
    let mut word = Word::default();

    if chars.get(index) == Some(&'@') {
        lexed.hazard(Hazard::Expression);
        word.dynamic = true;
    }

    while let Some(&c) = chars.get(index) {
        if BREAKS.contains(&c) || c == '$' && chars.get(index + 1) == Some(&'(') {
            break;
        }

        match c {
            '\'' => index = single(chars, index + 1, &mut word, lexed),
            '"' => index = double(chars, index + 1, &mut word, lexed),

            '`' => {
                lexed.hazard(Hazard::Escape);
                word.quoted = true;
                word.text.extend(chars.get(index + 1));
                index += 2;
            }

            '$' if chars.get(index + 1) == Some(&'{') => {
                let length = chars[index..].iter().take_while(|c| **c != '}').count();
                let end = (index + length + 1).min(chars.len());

                lexed.hazard(Hazard::Expression);
                word.dynamic = true;
                word.text.extend(&chars[index..end]);
                index = end;
            }

            '$' => {
                word.dynamic = true;
                word.text.push(c);
                index += 1;
            }

            '[' | ']' => {
                lexed.hazard(Hazard::Expression);
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
        lexed.hazard(Hazard::StopParsing);
    }

    (word, index)
}

fn single(chars: &[char], mut index: usize, word: &mut Word, lexed: &mut Lexed) -> usize {
    word.quoted = true;

    loop {
        match (chars.get(index), chars.get(index + 1)) {
            (None, _) => {
                lexed.hazard(Hazard::Unclosed);
                return index;
            }

            (Some('\''), Some('\'')) => {
                word.text.push('\'');
                index += 2;
            }

            (Some('\''), _) => return index + 1,

            (Some(&c), _) => {
                word.text.push(c);
                index += 1;
            }
        }
    }
}

fn double(chars: &[char], mut index: usize, word: &mut Word, lexed: &mut Lexed) -> usize {
    word.quoted = true;

    loop {
        match (chars.get(index), chars.get(index + 1)) {
            (None, _) => {
                lexed.hazard(Hazard::Unclosed);
                return index;
            }

            (Some('"'), Some('"')) => {
                word.text.push('"');
                index += 2;
            }

            (Some('"'), _) => return index + 1,

            (Some(&c), _) => {
                if matches!(c, '$' | '`') {
                    lexed.hazard(Hazard::Interpolation);
                    word.dynamic = true;
                }

                word.text.push(c);
                index += 1;
            }
        }
    }
}
