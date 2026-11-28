use super::lex::{self, Word};
use super::reason::{Hazard, Reason};
use super::{Dialect, rules, wrapper};

const LIMIT: usize = 8 * 1024;
const DEPTH: usize = 3;
const SEGMENTS: usize = 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parse {
    pub segments: Vec<Segment>,
    pub hazards: Vec<Hazard>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub dialect: Dialect,
    pub depth: usize,
    pub elevated: bool,
    pub wrappers: Vec<String>,
    pub program: String,
    pub args: Vec<String>,
    pub outputs: Vec<String>,
    pub inputs: Vec<String>,
    pub(super) head: Option<Word>,
    pub(super) words: Vec<Word>,
    pub(super) prefix: Vec<Word>,
    pub(super) targets: Vec<Word>,
    pub(super) sources: Vec<Word>,
    pub(super) assigns: bool,
    pub(super) exact: bool,
    pub(super) notes: Vec<Reason>,
}

pub(super) enum Nested {
    Text(String, Dialect, bool),
    Words(Vec<Word>),
}

pub fn parse(command: &str, dialect: Dialect) -> Parse {
    let mut parse = Parse::default();
    let command = command.trim();

    if command.is_empty() {
        parse.hazard(Hazard::Empty);
    } else if command.len() > LIMIT {
        parse.hazard(Hazard::Long);
    } else if command.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        parse.hazard(Hazard::Control);
    } else {
        parse.read(command, dialect, 0, false);
    }

    parse
}

impl Parse {
    fn hazard(&mut self, hazard: Hazard) {
        if !self.hazards.contains(&hazard) {
            self.hazards.push(hazard);
        }
    }

    fn read(&mut self, command: &str, dialect: Dialect, depth: usize, elevated: bool) {
        let lexed = match dialect {
            Dialect::Sh => lex::sh::lex(command),
            Dialect::Cmd => lex::cmd::lex(command),
            Dialect::PowerShell => lex::powershell::lex(command),
        };

        let (simples, hazards) = lexed.split();

        for hazard in hazards {
            self.hazard(hazard);
        }

        for simple in simples {
            self.build(Segment::new(simple, dialect, depth, elevated));
        }
    }

    fn build(&mut self, segment: Segment) {
        if self.segments.len() >= SEGMENTS {
            self.hazard(Hazard::Long);
            return;
        }

        let nested = rules::nested(&segment);
        let (dialect, depth, elevated) = (segment.dialect, segment.depth + 1, segment.elevated);

        self.segments.push(segment);

        if nested.is_empty() {
            return;
        }

        if depth > DEPTH {
            self.hazard(Hazard::Deep);
            return;
        }

        for nested in nested {
            match nested {
                Nested::Text(text, dialect, elevates) => {
                    self.read(&text, dialect, depth, elevated || elevates);
                }

                Nested::Words(words) => {
                    let simple = lex::Simple { words, ..lex::Simple::default() };

                    self.build(Segment::new(simple, dialect, depth, elevated));
                }
            }
        }
    }
}

impl Segment {
    fn new(simple: lex::Simple, dialect: Dialect, depth: usize, elevated: bool) -> Self {
        let lex::Simple { words, outputs, inputs } = simple;
        let mut segment = Self {
            dialect,
            depth,
            elevated,
            wrappers: Vec::new(),
            program: String::new(),
            args: Vec::new(),
            outputs: Vec::new(),
            inputs: inputs.iter().map(|word| word.text.clone()).collect(),
            head: None,
            words: Vec::new(),
            prefix: Vec::new(),
            targets: outputs.into_iter().filter(|word| !dialect.null(&word.text)).collect(),
            sources: inputs,
            assigns: false,
            exact: true,
            notes: Vec::new(),
        };

        let mut index = 0;

        while let Some(word) = words.get(index) {
            if dialect == Dialect::Sh && word.assignment {
                segment.assigns = true;
                segment.prefix.push(word.clone());
                index += 1;
                continue;
            }

            if word.dynamic || word.glob {
                break;
            }

            let (name, exact) = dialect.name(&word.text);
            let rest: Vec<&str> =
                words[index + 1..].iter().map(|word| word.text.as_str()).collect();

            let Some(unwrap) = wrapper::unwrap(dialect, &name, &rest) else {
                break;
            };

            if word.quoted {
                segment.notes.push(Reason::ProgramQuoted(name.clone()));
            }

            if !exact {
                segment.notes.push(Reason::ProgramPath(word.text.clone()));
            }

            if unwrap.elevated {
                segment.elevated = true;
                segment.notes.push(Reason::Elevated(name.clone()));
            }

            segment.assigns |= unwrap.assigns;
            segment.notes.extend(unwrap.notes);
            segment.prefix.extend_from_slice(&words[index..=index + unwrap.skip]);
            segment.wrappers.push(name);
            index += 1 + unwrap.skip;
        }

        if let Some(head) = words.get(index) {
            let (program, exact) = dialect.name(&head.text);

            segment.program = program;
            segment.exact = exact;
            segment.head = Some(head.clone());
            segment.words = words[index + 1..].to_vec();
        }

        segment.args = segment.words.iter().map(|word| word.text.clone()).collect();
        segment.outputs = segment.targets.iter().map(|word| word.text.clone()).collect();
        segment
    }

    pub fn operands(&self) -> Vec<&str> {
        let windows = self.dialect != Dialect::Sh;
        let mut operands = Vec::new();
        let mut ended = false;

        for arg in &self.args {
            if !ended && arg == "--" {
                ended = true;
            } else if ended || !(arg.starts_with('-') || switch(arg, windows)) {
                operands.push(arg.as_str());
            }
        }

        operands
    }
}

fn switch(arg: &str, windows: bool) -> bool {
    let Some(rest) = arg.strip_prefix('/') else {
        return false;
    };

    let letters = !rest.is_empty()
        && rest.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '?' | ':' | '-'));

    letters && (rest.len() == 1 || windows && !rest.contains('/'))
}

impl Dialect {
    pub(super) fn null(self, target: &str) -> bool {
        match self {
            Self::Sh => target == "/dev/null",
            Self::Cmd => target.eq_ignore_ascii_case("nul"),
            Self::PowerShell => target.eq_ignore_ascii_case("$null"),
        }
    }

    pub(super) fn name(self, text: &str) -> (String, bool) {
        match self {
            Self::Sh => match text.rsplit_once('/') {
                Some((_, base)) => (base.to_owned(), false),
                None => (text.to_owned(), true),
            },

            Self::Cmd | Self::PowerShell => {
                let mut name = text.to_ascii_lowercase();
                let mut exact = true;

                if self == Self::Cmd
                    && let Some(end) = name.find(['/', ',', ';', '='])
                {
                    name.truncate(end);
                    exact = false;
                }

                if let Some((_, base)) = name.rsplit_once(['\\', '/', ':']) {
                    name = base.to_owned();
                    exact = false;
                }

                for extension in [".exe", ".com", ".bat", ".cmd", ".ps1"] {
                    if let Some(stem) = name.strip_suffix(extension) {
                        name = stem.to_owned();
                        exact = false;
                    }
                }

                (name, exact)
            }
        }
    }
}
