pub(super) mod cmd;
pub(super) mod powershell;
pub(super) mod sh;

use super::Hazard;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Word {
    pub text: String,
    pub quoted: bool,
    pub dynamic: bool,
    pub glob: bool,
    pub assignment: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Token {
    Word(Word),
    Separator,
    Boundary,
    Output,
    Input,
}

#[derive(Debug, Default)]
pub(super) struct Lexed {
    pub tokens: Vec<Token>,
    pub hazards: Vec<Hazard>,
}

#[derive(Debug, Default)]
pub(super) struct Simple {
    pub words: Vec<Word>,
    pub outputs: Vec<Word>,
    pub inputs: Vec<Word>,
}

impl Lexed {
    pub fn push(&mut self, token: Token) {
        self.tokens.push(token);
    }

    pub fn hazard(&mut self, hazard: Hazard) {
        if !self.hazards.contains(&hazard) {
            self.hazards.push(hazard);
        }
    }

    pub fn word(&mut self, word: Word, end: Option<&char>, star: bool) {
        if !word.quoted && word.text.is_empty() {
            return;
        }

        if !word.quoted && matches!(end, Some('<' | '>')) {
            let digits = word.text.chars().all(|c| c.is_ascii_digit());

            if digits && word.text.len() == 1 || star && word.text == "*" {
                return;
            }

            if digits {
                self.hazard(Hazard::Redirection);
            }
        }

        self.tokens.push(Token::Word(word));
    }

    pub fn split(mut self) -> (Vec<Simple>, Vec<Hazard>) {
        let mut simples = Vec::new();
        let mut current = Simple::default();
        let mut started = false;
        let mut separated = false;
        let mut tokens = std::mem::take(&mut self.tokens).into_iter().peekable();

        while let Some(token) = tokens.next() {
            match token {
                Token::Word(word) => {
                    current.words.push(word);
                    started = true;
                }

                Token::Output | Token::Input => {
                    let Some(Token::Word(target)) = tokens.next_if(|t| matches!(t, Token::Word(_)))
                    else {
                        self.hazard(Hazard::Target);
                        continue;
                    };

                    if token == Token::Output {
                        current.outputs.push(target);
                    } else {
                        current.inputs.push(target);
                    }

                    started = true;
                }

                Token::Separator | Token::Boundary => {
                    if started {
                        simples.push(std::mem::take(&mut current));
                    } else if token == Token::Separator {
                        self.hazard(Hazard::EmptyPart);
                    }

                    started = false;
                    separated = token == Token::Separator;
                    continue;
                }
            }

            separated = false;
        }

        if started {
            simples.push(current);
        } else if separated {
            self.hazard(Hazard::EmptyPart);
        }

        (simples, self.hazards)
    }
}
