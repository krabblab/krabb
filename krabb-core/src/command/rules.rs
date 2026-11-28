mod powershell;
mod unix;
mod windows;

use super::lex::Word;
use super::package::{self, Packages};
use super::parse::{Nested, Segment};
use super::reason::{Effect, Hazard, Reason};
use super::{Dialect, git, path};

const ELEVATORS: &[&str] = &["sudo", "doas", "su", "runas", "pkexec", "gsudo"];
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "mksh", "ash", "fish"];
const SCRIPTS: &[&str] = &["npm", "pnpm", "yarn", "bun"];

const CODE: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "dash",
    "ksh",
    "mksh",
    "ash",
    "fish",
    "csh",
    "tcsh",
    "eval",
    "exec",
    "source",
    ".",
    "python",
    "python3",
    "py",
    "node",
    "deno",
    "ruby",
    "perl",
    "php",
    "awk",
    "gawk",
    "sed",
    "cmd",
    "powershell",
    "pwsh",
    "iex",
    "invoke-expression",
    "invoke-command",
    "icm",
];

const NETWORK: &[&str] = &[
    "curl",
    "wget",
    "nc",
    "ncat",
    "netcat",
    "ssh",
    "scp",
    "sftp",
    "rsync",
    "ping",
    "dig",
    "nslookup",
    "host",
    "telnet",
    "ftp",
    "whois",
    "aria2c",
    "invoke-webrequest",
    "iwr",
    "invoke-restmethod",
    "irm",
    "test-netconnection",
    "start-bitstransfer",
    "bitsadmin",
];

const SERVICE_QUERIES: &[&str] = &[
    "status",
    "show",
    "list-units",
    "list-unit-files",
    "list-timers",
    "list-sockets",
    "list-dependencies",
    "is-active",
    "is-enabled",
    "is-failed",
    "cat",
    "list",
    "print",
    "print-disabled",
    "blame",
    "version",
    "help",
    "query",
    "queryex",
    "qc",
    "qdescription",
    "qfailure",
    "getdisplayname",
    "getkeyname",
    "enumdepend",
    "--status-all",
];

const DRIVES: &[&str] =
    &["env:", "hklm:", "hkcu:", "cert:", "variable:", "function:", "alias:", "wsman:"];

const PROVIDERS: &[&str] = &[
    "registry::",
    "environment::",
    "certificate::",
    "variable::",
    "function::",
    "alias::",
    "wsman::",
];

#[derive(Debug, Default)]
pub(super) struct Findings {
    pub reasons: Vec<Reason>,
    pub packages: Option<Packages>,
}

impl Findings {
    pub fn push(&mut self, reason: Reason) {
        if !self.reasons.contains(&reason) {
            self.reasons.push(reason);
        }
    }

    pub fn acts(&mut self, command: impl Into<String>, effect: Effect) {
        self.push(Reason::Acts(command.into(), effect));
    }
}

pub(super) fn evaluate(segment: &Segment, found: &mut Findings) {
    for note in &segment.notes {
        found.push(note.clone());
    }

    if segment.prefix.iter().any(|word| word.dynamic) {
        found.push(Reason::Variable);
    }

    if segment.prefix.iter().any(|word| word.glob) {
        found.push(Reason::ProgramWildcard);
    }

    match &segment.head {
        None => found.push(Reason::Missing),
        Some(head) if head.dynamic => found.push(Reason::ProgramVariable),
        Some(head) if head.glob => found.push(Reason::ProgramWildcard),
        Some(head) => program(segment, head, found),
    }

    if segment.assigns {
        found.push(Reason::Assigns);
    }

    for target in &segment.targets {
        found.push(Reason::Redirects(target.text.clone()));
    }

    for source in &segment.sources {
        if source.dynamic {
            found.push(Reason::Variable);
        }

        if source.glob {
            found.push(Reason::Hazard(Hazard::Redirection));
        }
    }

    let words = segment.prefix.iter().chain(&segment.words).chain(&segment.targets);

    for word in words.chain(&segment.sources) {
        location(segment.dialect, &word.text, found);
    }
}

fn program(segment: &Segment, head: &Word, found: &mut Findings) {
    let name = segment.program.as_str();
    let args: Vec<&str> = segment.args.iter().map(String::as_str).collect();

    if name.is_empty() {
        found.acts(head.text.clone(), Effect::Unlisted);
        return;
    }

    if head.quoted {
        found.push(Reason::ProgramQuoted(name.to_owned()));
    }

    if !segment.exact {
        found.push(Reason::ProgramPath(head.text.clone()));
    }

    let handled = match segment.dialect {
        Dialect::Sh => unix::evaluate(name, &args, found),
        Dialect::Cmd => windows::evaluate(name, &args, found),
        Dialect::PowerShell => powershell::evaluate(name, &args, found),
    };

    if !handled && !portable(name, &args, found) {
        found.acts(name, Effect::Unlisted);
    }

    if segment.words.iter().any(|word| word.dynamic) {
        found.push(Reason::Variable);
    }

    let inert = segment.dialect == Dialect::Sh && unix::inert(name);

    if !inert && segment.words.iter().any(|word| word.glob) {
        found.push(Reason::Wildcard(name.to_owned()));
    }
}

fn portable(name: &str, args: &[&str], found: &mut Findings) -> bool {
    if name == "git" {
        git::evaluate(args, found);
    } else if ELEVATORS.contains(&name) {
        found.push(Reason::Elevated(name.to_owned()));

        if args.iter().any(|arg| matches!(*arg, "-e" | "--edit")) {
            found.acts(format!("{name} -e"), Effect::ChangesFiles);
        } else if name == "su" && !args.iter().any(|arg| matches!(*arg, "-c" | "--command")) {
            found.acts(name, Effect::RunsCode);
        } else if !matches!(name, "su" | "runas") {
            found.acts(name, Effect::Unlisted);
        }
    } else if let Some(packages) = package::read(name, args) {
        found.acts(packages.manager.clone(), Effect::Packages(packages.action));
        found.packages.get_or_insert(packages);
    } else if package::manager(name, args) {
        let effect = if SCRIPTS.contains(&name) { Effect::RunsCode } else { Effect::Unlisted };

        found.acts(name, effect);
    } else if matches!(name, "systemctl" | "service" | "launchctl" | "sc") {
        let query = args.is_empty() && name == "systemctl"
            || args.iter().any(|arg| SERVICE_QUERIES.contains(&arg.to_ascii_lowercase().as_str()));

        let effect = match query {
            true => Effect::QueriesServices,
            false => Effect::ManagesServices,
        };

        found.acts(name, effect);
    } else if CODE.contains(&name) {
        found.acts(name, Effect::RunsCode);
    } else if NETWORK.contains(&name) {
        found.acts(name, Effect::UsesNetwork);
    } else {
        return false;
    }

    true
}

fn location(dialect: Dialect, text: &str, found: &mut Findings) {
    let value = text.split_once(':').filter(|_| text.starts_with('-')).map_or(text, |(_, v)| v);

    for candidate in [text, value] {
        if let Some(host) = path::host(candidate) {
            found.push(Reason::Host(host.to_owned()));
        } else if candidate.contains("://") {
            found.push(Reason::Host(candidate.to_owned()));
        }

        if path::device(candidate) {
            found.push(Reason::Device(candidate.to_owned()));
        }

        if dialect == Dialect::PowerShell {
            let lowered = candidate.to_ascii_lowercase();
            let drive = DRIVES.iter().find(|drive| lowered.starts_with(*drive));
            let provider = PROVIDERS.iter().find(|provider| lowered.contains(*provider));

            if let Some(drive) = drive.or(provider) {
                found.push(Reason::Drive(drive.trim_end_matches(':').to_owned()));
            }
        }
    }
}

pub(super) fn nested(segment: &Segment) -> Vec<Nested> {
    let name = segment.program.as_str();
    let args = &segment.args;
    let lowered: Vec<String> = args.iter().map(|arg| arg.to_ascii_lowercase()).collect();
    let after = |at: usize| args[at + 1..].join(" ");

    let found = match name {
        _ if segment.dialect == Dialect::Sh && SHELLS.contains(&name) => args
            .iter()
            .position(|arg| arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c'))
            .and_then(|at| args.get(at + 1))
            .map(|text| Nested::Text(text.clone(), Dialect::Sh, false)),

        "su" => args
            .iter()
            .position(|arg| matches!(arg.as_str(), "-c" | "--command"))
            .and_then(|at| args.get(at + 1))
            .map(|text| Nested::Text(text.clone(), Dialect::Sh, true)),

        "eval" if segment.dialect == Dialect::Sh => {
            Some(Nested::Text(args.join(" "), Dialect::Sh, false))
        }

        "cmd" => lowered
            .iter()
            .position(|arg| matches!(arg.as_str(), "/c" | "/k" | "/r"))
            .map(|at| Nested::Text(after(at), Dialect::Cmd, false)),

        "powershell" | "pwsh" => lowered
            .iter()
            .position(|arg| arg.len() > 1 && "-command".starts_with(arg.as_str()))
            .map(|at| Nested::Text(after(at), Dialect::PowerShell, false)),

        "iex" | "invoke-expression" if segment.dialect == Dialect::PowerShell => {
            let text: Vec<&str> =
                args.iter().filter(|arg| !arg.starts_with('-')).map(String::as_str).collect();

            Some(Nested::Text(text.join(" "), Dialect::PowerShell, false))
        }

        "runas" => args
            .iter()
            .rev()
            .find(|arg| !arg.starts_with('/'))
            .map(|text| Nested::Text(text.clone(), Dialect::Cmd, true)),

        _ => None,
    };

    if let Some(found) = found {
        return vec![found];
    }

    if segment.dialect != Dialect::Sh {
        return Vec::new();
    }

    match name {
        "find" => unix::executions(&segment.words).into_iter().map(Nested::Words).collect(),

        "xargs" => match unix::xargs_command(&segment.args) {
            Ok(start) if start < segment.words.len() => {
                vec![Nested::Words(segment.words[start..].to_vec())]
            }

            _ => Vec::new(),
        },

        _ => Vec::new(),
    }
}
