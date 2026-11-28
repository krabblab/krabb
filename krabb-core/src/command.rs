mod admin;
mod git;
mod lex;
mod option;
mod package;
mod parse;
mod path;
mod reason;
mod rules;
mod target;
mod wrapper;

#[cfg(test)]
mod tests;

use std::cmp::Reverse;

pub use admin::administers;
pub use package::{Action, Packages};
pub use parse::{Parse, Segment, parse};
pub use path::{Os, device, host, outside, remote, share_host, system, url_host};
pub use reason::{Effect, Hazard, Reason};
pub use target::{Targets, targets};

const REASONS: usize = 8;

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
    Services,
    Installs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub kind: Kind,
    pub elevated: bool,
    pub reasons: Vec<Reason>,
    pub packages: Option<Packages>,
}

impl Verdict {
    pub fn is_read_only(&self) -> bool {
        self.kind == Kind::ReadOnly && !self.elevated
    }
}

pub fn classify(command: &str, dialect: Dialect) -> Verdict {
    parse(command, dialect).verdict()
}

impl Parse {
    /// The verdict with its strongest reasons, at most a few, for people to read.
    pub fn verdict(&self) -> Verdict {
        let mut verdict = self.findings();

        verdict.reasons.truncate(REASONS);
        verdict
    }

    /// The verdict with every reason, strongest first, for callers that describe the command.
    pub fn findings(&self) -> Verdict {
        let mut found = rules::Findings::default();

        for hazard in &self.hazards {
            found.push(Reason::Hazard(*hazard));
        }

        for segment in &self.segments {
            rules::evaluate(segment, &mut found);
        }

        let mut reasons = found.reasons;
        let kind = reasons.iter().filter_map(Reason::kind).max().unwrap_or(Kind::Unknown);

        let elevated = self.segments.iter().any(|segment| segment.elevated)
            || reasons.iter().any(|reason| matches!(reason, Reason::Elevated(_)));

        reasons.sort_by_key(|reason| (reason.kind().is_some(), Reverse(reason.kind())));

        Verdict { kind, elevated, reasons, packages: found.packages }
    }
}
