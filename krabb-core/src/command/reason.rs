use std::fmt;

use super::Kind;
use super::package::Action;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hazard {
    Empty,
    Long,
    Control,
    Deep,
    Substitution,
    ProcessSubstitution,
    Subshell,
    Background,
    Heredoc,
    Comment,
    CaseTerminator,
    Expansion,
    SpecialQuote,
    Unclosed,
    Backslash,
    EmptyPart,
    Target,
    Redirection,
    Escape,
    Variable,
    Delayed,
    Group,
    Lines,
    Call,
    Reserved,
    Expression,
    StopParsing,
    Typographic,
    Interpolation,
    Spacing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Reads,
    ChangesFiles,
    WritesFile,
    DeletesFiles,
    CreatesBranch,
    ChangesBranches,
    DeletesBranches,
    ChangesRepository,
    ChangesConfiguration,
    SetsClock,
    ManagesServices,
    QueriesServices,
    Packages(Action),
    RunsCode,
    RunsPrograms,
    RunsConfigured,
    UsesNetwork,
    SetsVariable,
    ChangesEnvironment,
    ReceivesOptions,
    ShowsEnvironment,
    Waits,
    ReadsNamed,
    Unlisted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    Hazard(Hazard),
    Acts(String, Effect),
    Elevated(String),
    Redirects(String),
    Assigns,
    Missing,
    ProgramVariable,
    ProgramWildcard,
    ProgramQuoted(String),
    ProgramPath(String),
    Variable,
    Wildcard(String),
    Host(String),
    Device(String),
    Drive(String),
    Outside(String),
    Pattern(String),
}

impl Effect {
    pub fn kind(self) -> Kind {
        match self {
            Self::Reads => Kind::ReadOnly,

            Self::ChangesFiles
            | Self::WritesFile
            | Self::CreatesBranch
            | Self::ChangesBranches
            | Self::ChangesRepository
            | Self::ChangesConfiguration
            | Self::SetsClock => Kind::Writes,

            Self::DeletesFiles | Self::DeletesBranches => Kind::Deletes,
            Self::ManagesServices => Kind::Services,
            Self::Packages(Action::Query) => Kind::Unknown,
            Self::Packages(_) => Kind::Installs,
            _ => Kind::Unknown,
        }
    }
}

impl Reason {
    pub fn kind(&self) -> Option<Kind> {
        match self {
            Self::Acts(_, effect) => Some(effect.kind()),
            Self::Elevated(_) => None,
            Self::Redirects(_) => Some(Kind::Writes),
            _ => Some(Kind::Unknown),
        }
    }
}

impl fmt::Display for Hazard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "the command is empty",
            Self::Long => "the command is too long to check",
            Self::Control => "the command has control characters",
            Self::Deep => "the command nests other commands too deeply to check",
            Self::Substitution => "the command runs a command substitution",
            Self::ProcessSubstitution => "the command uses process substitution",
            Self::Subshell => "the command opens a subshell",
            Self::Background => "the command runs something in the background",
            Self::Heredoc => "the command has a heredoc",
            Self::Comment => "the command has a comment",
            Self::CaseTerminator => "the command has a stray `;;`",
            Self::Expansion => "the command expands a parameter or arithmetic",
            Self::SpecialQuote => "the command uses `$'…'` or `$\"…\"` quoting",
            Self::Unclosed => "a quote is not closed",
            Self::Backslash => "the command ends with a backslash",
            Self::EmptyPart => "a part of the command is empty",
            Self::Target => "a redirection has no target",
            Self::Redirection => "the command has an unusual redirection",
            Self::Escape => "the command uses an escape character",
            Self::Variable => "the command expands an environment variable",
            Self::Delayed => "the command uses delayed expansion",
            Self::Group => "the command has a parenthesized group",
            Self::Lines => "the command has more than one line",
            Self::Call => "the command uses the call operator `&`",
            Self::Reserved => "the command uses a reserved operator",
            Self::Expression => "the command has an expression",
            Self::StopParsing => "the command uses `--%`",
            Self::Typographic => "the command has a typographic quote or dash",
            Self::Interpolation => "a double-quoted string expands variables",
            Self::Spacing => "the command has an unusual space character",
        })
    }
}

impl fmt::Display for Effect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Reads => "only reads",
            Self::ChangesFiles => "changes files",
            Self::WritesFile => "writes a file",
            Self::DeletesFiles => "deletes files",
            Self::CreatesBranch => "creates a branch",
            Self::ChangesBranches => "changes branches",
            Self::DeletesBranches => "deletes branches",
            Self::ChangesRepository => "changes the repository",
            Self::ChangesConfiguration => "changes the configuration",
            Self::SetsClock => "sets the clock",
            Self::ManagesServices => "starts, stops, or changes services",
            Self::QueriesServices => "looks up services",
            Self::Packages(Action::Install) => "installs packages",
            Self::Packages(Action::Remove) => "removes packages",
            Self::Packages(Action::Update) => "updates packages",
            Self::Packages(Action::Query) => "looks up packages",
            Self::RunsCode => "runs arbitrary code",
            Self::RunsPrograms => "runs other programs",
            Self::RunsConfigured => "can run programs the repository configures",
            Self::UsesNetwork => "uses the network",
            Self::SetsVariable => "sets a variable",
            Self::ChangesEnvironment => "runs a command with a changed environment",
            Self::ReceivesOptions => "could receive options from its input",
            Self::ShowsEnvironment => "can show secrets from the environment",
            Self::Waits => "keeps running until it is stopped",
            Self::ReadsNamed => "reads files named in its input or a list",
            Self::Unlisted => "is not on the read-only list",
        })
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hazard(hazard) => hazard.fmt(formatter),
            Self::Acts(command, effect) => write!(formatter, "`{command}` {effect}"),
            Self::Elevated(program) => write!(formatter, "`{program}` runs with admin rights"),

            Self::Redirects(target) => {
                write!(formatter, "the command writes its output to `{target}`")
            }

            Self::Assigns => formatter.write_str("the command sets a variable for the program"),
            Self::Missing => formatter.write_str("a part of the command names no program"),
            Self::ProgramVariable => formatter.write_str("the program name comes from a variable"),
            Self::ProgramWildcard => formatter.write_str("a wildcard could change the program"),
            Self::ProgramQuoted(name) => write!(formatter, "the program name `{name}` is quoted"),
            Self::ProgramPath(path) => write!(formatter, "the command runs `{path}` by path"),
            Self::Variable => formatter.write_str("an argument comes from a variable"),

            Self::Wildcard(program) => {
                write!(formatter, "a wildcard could pass options to `{program}`")
            }

            Self::Host(host) => {
                write!(formatter, "the command reaches the network location `{host}`")
            }

            Self::Device(device) => write!(formatter, "the command opens the device `{device}`"),
            Self::Drive(drive) => write!(formatter, "the command reaches the `{drive}` drive"),

            Self::Outside(path) => {
                write!(formatter, "the command reaches `{path}` outside the workspace")
            }

            Self::Pattern(word) => {
                write!(formatter, "the pattern `{word}` could reach outside the workspace")
            }
        }
    }
}
