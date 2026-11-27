use super::Dialect;

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
    Output,
    Input,
}

#[derive(Debug, Default)]
pub(super) struct Simple {
    pub words: Vec<Word>,
    pub assigns: bool,
    pub output: bool,
    started: bool,
}

pub(super) fn split(tokens: Vec<Token>, dialect: Dialect) -> Result<Vec<Simple>, &'static str> {
    let mut simples = Vec::new();
    let mut current = Simple::default();
    let mut tokens = tokens.into_iter();

    while let Some(token) = tokens.next() {
        match token {
            Token::Word(word) => {
                if current.words.is_empty() && word.assignment {
                    current.assigns = true;
                } else {
                    current.words.push(word);
                }
            }

            Token::Separator => {
                if !current.started {
                    return Err("a command part is empty");
                }

                simples.push(std::mem::take(&mut current));
                continue;
            }

            Token::Output | Token::Input => {
                let Some(Token::Word(target)) = tokens.next() else {
                    return Err("a redirection has no target");
                };

                if token == Token::Output && !dialect.null(&target.text) {
                    current.output = true;
                }
            }
        }

        current.started = true;
    }

    if !current.started {
        return Err("a command part is empty");
    }

    simples.push(current);
    Ok(simples)
}

pub(super) fn long(arg: &str, option: &str) -> bool {
    let Some(name) = arg.strip_prefix("--") else {
        return false;
    };

    let name = name.split_once('=').map_or(name, |(name, _)| name);
    !name.is_empty() && option.starts_with(name)
}

pub(super) fn operands(args: &[&str], values: &[&str]) -> usize {
    let mut count = 0;
    let mut index = 0;
    let mut options = true;

    while let Some(arg) = args.get(index) {
        index += 1;

        if options && *arg == "--" {
            options = false;
        } else if options && arg.len() > 1 && arg.starts_with('-') {
            if values.contains(arg) {
                index += 1;
            }
        } else {
            count += 1;
        }
    }

    count
}
