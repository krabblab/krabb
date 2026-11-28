use super::{Lexed, Token, Word};
use crate::command::Hazard;

const BREAKS: &[char] = &[' ', '\t', '\n', '&', '|', '<', '>', '(', ')'];

pub(in crate::command) fn lex(input: &str) -> Lexed {
    let chars: Vec<char> = input.chars().collect();
    let mut lexed = Lexed::default();
    let mut index = 0;

    while let Some(&c) = chars.get(index) {
        let next = chars.get(index + 1).copied();

        match c {
            ' ' | '\t' => index += 1,

            '\n' => {
                lexed.hazard(Hazard::Lines);
                lexed.push(Token::Separator);
                index += 1;
            }

            '(' | ')' => {
                lexed.hazard(Hazard::Group);
                lexed.push(Token::Boundary);
                index += 1;
            }

            '&' | '|' => {
                lexed.push(Token::Separator);
                index += 1 + usize::from(next == Some(c));
            }

            '<' | '>' if next == Some('&') => {
                let digits = chars[index + 2..].iter().take_while(|c| c.is_ascii_digit()).count();
                let bounded = chars.get(index + 2 + digits).is_none_or(|c| BREAKS.contains(c));

                if digits == 0 || !bounded {
                    lexed.hazard(Hazard::Redirection);
                    lexed.push(if c == '<' { Token::Input } else { Token::Output });

                    index += 2;
                    continue;
                }

                index += 2 + digits;
            }

            '<' => {
                lexed.push(Token::Input);
                index += 1;
            }

            '>' => {
                lexed.push(Token::Output);
                index += 1 + usize::from(next == Some('>'));
            }

            _ => {
                let (word, end) = word(&chars, index, &mut lexed);

                lexed.word(word, chars.get(end), false);
                index = end;
            }
        }
    }

    lexed
}

fn word(chars: &[char], mut index: usize, lexed: &mut Lexed) -> (Word, usize) {
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

            '^' if !quoted => {
                lexed.hazard(Hazard::Escape);
                word.quoted = true;
                index += 1;
                word.text.extend(chars.get(index));
            }

            '%' => {
                lexed.hazard(Hazard::Variable);
                word.dynamic = true;
                word.text.push(c);
            }

            '!' => {
                lexed.hazard(Hazard::Delayed);
                word.dynamic = true;
                word.text.push(c);
            }

            '\n' => {
                lexed.hazard(Hazard::Lines);
                word.text.push(c);
            }

            _ => word.text.push(c),
        }

        index += 1;
    }

    if quoted {
        lexed.hazard(Hazard::Unclosed);
    }

    (word, index)
}
