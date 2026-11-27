mod cmd;
mod git;
mod powershell;
mod program;
mod sh;
mod syntax;

#[cfg(test)]
mod tests;

use syntax::{Simple, Token, Word};

const LIMIT: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialect {
    Sh,
    Cmd,
    PowerShell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    ReadOnly,
    Unknown,
    Writes,
    Deletes,
    Installs,
    Elevates,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub kind: Kind,
    pub reason: String,
}

impl Verdict {
    fn new(kind: Kind, reason: impl Into<String>) -> Self {
        Self { kind, reason: reason.into() }
    }

    pub fn is_read_only(&self) -> bool {
        self.kind == Kind::ReadOnly
    }

    fn cap(self, reason: &str) -> Self {
        if self.is_read_only() { Self::new(Kind::Unknown, reason) } else { self }
    }

    fn max(self, other: Self) -> Self {
        if other.kind > self.kind { other } else { self }
    }
}

pub fn classify(command: &str, dialect: Dialect) -> Verdict {
    let command = command.trim();

    if command.is_empty() {
        return Verdict::new(Kind::Unknown, "the command is empty");
    }

    if command.len() > LIMIT {
        return Verdict::new(Kind::Unknown, "the command is too long to check");
    }

    if command.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Verdict::new(Kind::Unknown, "the command has control characters");
    }

    let simples = match dialect.lex(command).and_then(|tokens| syntax::split(tokens, dialect)) {
        Ok(simples) => simples,
        Err(reason) => return Verdict::new(Kind::Unknown, reason),
    };

    simples
        .iter()
        .map(|simple| classify_simple(dialect, simple))
        .reduce(Verdict::max)
        .unwrap_or_else(|| Verdict::new(Kind::Unknown, "the command is empty"))
}

fn classify_simple(dialect: Dialect, simple: &Simple) -> Verdict {
    let mut verdict = match simple.words.split_first() {
        Some((head, args)) => classify_program(dialect, head, args),
        None => Verdict::new(Kind::Unknown, "a command part has no program"),
    };

    if simple.assigns {
        verdict = verdict.cap("the command sets a variable");
    }

    if simple.output {
        verdict =
            verdict.max(Verdict::new(Kind::Writes, "the command redirects output into a file"));
    }

    verdict
}

fn classify_program(dialect: Dialect, head: &Word, args: &[Word]) -> Verdict {
    if head.dynamic {
        return Verdict::new(Kind::Unknown, "the program name comes from a variable");
    }

    if head.glob {
        return Verdict::new(Kind::Unknown, "the program name has a wildcard");
    }

    let (name, exact) = dialect.name(&head.text);

    if name.is_empty() {
        return Verdict::new(Kind::Unknown, "the program name is unusual");
    }

    let values: Vec<&str> = args.iter().map(|word| word.text.as_str()).collect();
    let mut verdict = dialect.program(&name, &values);

    if head.quoted || !exact {
        verdict = verdict.cap("the program name is quoted or a path");
    }

    if args.iter().any(|word| word.dynamic) {
        verdict = verdict.cap("an argument expands a variable");
    }

    if args.iter().any(|word| word.glob) && !sh::open(&name) {
        verdict = verdict.cap("a wildcard could add options");
    }

    verdict
}

impl Dialect {
    fn lex(self, command: &str) -> Result<Vec<Token>, &'static str> {
        match self {
            Self::Sh => sh::lex(command),
            Self::Cmd => cmd::lex(command),
            Self::PowerShell => powershell::lex(command),
        }
    }

    fn program(self, name: &str, args: &[&str]) -> Verdict {
        match self {
            Self::Sh => sh::program(name, args),
            Self::Cmd => cmd::program(name, args),
            Self::PowerShell => powershell::program(name, args),
        }
    }

    fn null(self, target: &str) -> bool {
        match self {
            Self::Sh => target == "/dev/null",
            Self::Cmd => target.eq_ignore_ascii_case("nul"),
            Self::PowerShell => target.eq_ignore_ascii_case("$null"),
        }
    }

    fn name(self, text: &str) -> (String, bool) {
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

                if let Some(stem) = name.strip_suffix(".exe") {
                    name = stem.to_owned();
                    exact = false;
                }

                (name, exact)
            }
        }
    }
}
