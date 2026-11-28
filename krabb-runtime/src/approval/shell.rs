//! Words a shell command's fact line from the reading in `krabb_core::command`.
//!
//! The core parses and classifies the command; this module only chooses the shell it runs in
//! and phrases the result. The exact command is always shown below the fact line.

use krabb_core::command::{self, Action, Dialect, Effect, Os, Parse, Reason};

use super::facts::{list, sentence, visible};

const LOCAL: &[&str] = &["msiexec", "dpkg"];

#[derive(Default)]
struct Reading {
    deletes: Vec<String>,
    unnamed_deletes: bool,
    recursive: bool,
    writes: Vec<String>,
    unnamed_writes: bool,
    changes: Vec<String>,
    hosts: Vec<String>,
    network: bool,
    admin: bool,
    opaque: bool,
    unchecked: bool,
}

/// The fact line for COMMAND as the host would run it.
pub(crate) fn fact(command: &str) -> String {
    let (dialect, os) = place();

    describe(command, dialect, os)
}

/// The shell and system a command runs in: a container runs `sh` on Linux; otherwise the host
/// shell runs on the host's system.
fn place() -> (Dialect, Os) {
    let contained = std::env::var("KRABB_SANDBOX_RUNTIME")
        .is_ok_and(|runtime| matches!(runtime.as_str(), "docker" | "podman"));

    if contained {
        (Dialect::Sh, Os::Linux)
    } else if cfg!(windows) {
        (Dialect::Cmd, Os::Windows)
    } else if cfg!(target_os = "macos") {
        (Dialect::Sh, Os::MacOs)
    } else {
        (Dialect::Sh, Os::Linux)
    }
}

/// The fact line for COMMAND read in DIALECT on OS.
pub(crate) fn describe(command: &str, dialect: Dialect, os: Os) -> String {
    let parse = command::parse(command, dialect);
    let verdict = parse.verdict();

    let mut reading = Reading {
        admin: verdict.elevated,
        unchecked: !parse.hazards.is_empty(),
        ..Reading::default()
    };

    for (index, segment) in parse.segments.iter().enumerate() {
        let single = Parse { segments: vec![segment.clone()], hazards: Vec::new() }.verdict();
        let read = parse.segments.get(index + 1).is_some_and(|next| next.depth > segment.depth);
        let targets = command::targets(segment);

        reading.admin |= single.elevated || command::administers(segment);
        reading.recursive |= targets.recursive;
        reading.unnamed_writes |= targets.unnamed_writes;
        reading.unnamed_deletes |= targets.unnamed_deletes;

        for target in targets.writes {
            reading.admin |= command::system(&target, os);
            push(&mut reading.writes, target);
        }

        for target in targets.deletes {
            reading.admin |= command::system(&target, os);
            push(&mut reading.deletes, target);
        }

        for reason in single.reasons {
            match reason {
                Reason::Host(host) if !host.contains("://") => {
                    reading.network = true;
                    push(&mut reading.hosts, host);
                }

                Reason::Acts(_, Effect::UsesNetwork) => reading.network = true,
                Reason::Acts(_, Effect::RunsCode) => reading.opaque |= !read && !scripted(segment),
                Reason::Acts(_, Effect::DeletesFiles) => reading.unnamed_deletes = true,
                Reason::Acts(_, effect) => changed(&mut reading, effect),

                _ => {}
            }
        }
    }

    reading.unnamed_deletes &= reading.deletes.is_empty();

    let packages = verdict.packages.filter(|packages| packages.action != Action::Query);

    let kind = match &packages {
        Some(packages) => {
            reading.network |=
                packages.action != Action::Remove && !LOCAL.contains(&&*packages.manager);
            reading.admin |= packages.system;

            let verb = match packages.action {
                Action::Install => "Install",
                Action::Remove => "Remove",
                Action::Update | Action::Query => "Update",
            };

            let names = match packages.names.is_empty() {
                true => String::new(),
                false => format!(": {}", list(&packages.names)),
            };

            let manager = match packages.manager.as_str() {
                "powershell" => "PowerShell".into(),
                manager => visible(manager),
            };

            format!("{verb} packages with {manager}{names}")
        }

        None if !reading.deletes.is_empty() || reading.unnamed_deletes => {
            let what = if reading.recursive { "files and folders" } else { "files" };

            match reading.deletes.is_empty() {
                true => format!("Delete {what}"),
                false => format!("Delete {what}: {}", list(&reading.deletes)),
            }
        }

        None => "Run a shell command".into(),
    };

    sentence(&kind, &flags(&reading, packages.is_some()))
}

/// Whether SEGMENT is a stream editor whose whole script is in the command shown below.
fn scripted(segment: &command::Segment) -> bool {
    matches!(segment.program.as_str(), "sed" | "awk" | "gawk")
        && !segment.args.iter().any(|arg| arg == "-f" || arg.starts_with("--file"))
}

fn changed(reading: &mut Reading, effect: Effect) {
    match effect {
        Effect::ChangesFiles | Effect::WritesFile => reading.unnamed_writes = true,

        Effect::CreatesBranch
        | Effect::ChangesBranches
        | Effect::DeletesBranches
        | Effect::ChangesRepository
        | Effect::ChangesConfiguration
        | Effect::SetsClock
        | Effect::ManagesServices => push(&mut reading.changes, effect.to_string()),

        _ => {}
    }
}

fn flags(reading: &Reading, packages: bool) -> Vec<String> {
    let mut flags = Vec::new();

    if !reading.writes.is_empty() {
        flags.push(format!("writes {}", list(&reading.writes)));
    } else if reading.unnamed_writes {
        flags.push("writes files".into());
    }

    if packages && !reading.deletes.is_empty() {
        flags.push(format!("deletes {}", list(&reading.deletes)));
    } else if packages && reading.unnamed_deletes {
        flags.push("deletes files".into());
    }

    flags.extend(reading.changes.iter().cloned());

    if !reading.hosts.is_empty() {
        flags.push(format!("uses the network ({})", list(&reading.hosts)));
    } else if reading.network {
        flags.push("uses the network".into());
    }

    if reading.admin {
        flags.push("needs admin rights".into());
    }

    if reading.opaque {
        flags.push("runs code Krabb cannot read".into());
    } else if reading.unchecked {
        flags.push("has parts Krabb cannot check".into());
    }

    flags
}

fn push(items: &mut Vec<String>, item: String) {
    if !items.contains(&item) {
        items.push(item);
    }
}
