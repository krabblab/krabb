use super::Dialect;
use super::reason::{Effect, Reason};

const SUDO_VALUES: &[&str] = &[
    "-u",
    "-g",
    "-C",
    "-D",
    "-h",
    "-p",
    "-U",
    "-r",
    "-t",
    "-T",
    "-R",
    "--user",
    "--group",
    "--close-from",
    "--chdir",
    "--host",
    "--prompt",
    "--other-user",
    "--role",
    "--type",
    "--command-timeout",
    "--chroot",
];

const SUDO_ALONE: &[&str] =
    &["-e", "--edit", "-l", "--list", "-v", "--validate", "-k", "-K", "--remove-timestamp", "-V"];

const ENV_CHANGES: &[&str] = &["-i", "-", "--ignore-environment", "-0", "--null"];

const TIME_FLAGS: &[&str] =
    &["-p", "-v", "-a", "-q", "--portability", "--verbose", "--append", "--quiet"];

#[derive(Default)]
pub(super) struct Unwrap {
    pub skip: usize,
    pub elevated: bool,
    pub assigns: bool,
    pub notes: Vec<Reason>,
}

pub(super) fn unwrap(dialect: Dialect, name: &str, args: &[&str]) -> Option<Unwrap> {
    let unwrap = match (dialect, name) {
        (_, "sudo" | "gsudo") => elevate(sudo(args)?),
        (Dialect::Sh, "doas") => elevate(options(args, &["-u", "-C"])),
        (Dialect::Sh, "pkexec") => elevate(options(args, &["--user"])),
        (Dialect::Sh, "env") => env(args)?,
        (Dialect::Sh, "nice") => options(args, &["-n", "--adjustment"]),
        (Dialect::Sh, "timeout") => timeout(args),
        (Dialect::Sh, "time") => time(args),
        (Dialect::Sh, "command") => command(args)?,

        (Dialect::Sh, "nohup") => Unwrap {
            notes: vec![Reason::Acts("nohup".into(), Effect::WritesFile)],
            ..Unwrap::default()
        },

        _ => return None,
    };

    (unwrap.skip < args.len()).then_some(unwrap)
}

fn elevate(unwrap: Unwrap) -> Unwrap {
    Unwrap { elevated: true, ..unwrap }
}

fn options(args: &[&str], values: &[&str]) -> Unwrap {
    let mut index = 0;

    while let Some(&arg) = args.get(index) {
        if arg == "--" {
            index += 1;
            break;
        }

        if !arg.starts_with('-') || arg.len() == 1 {
            break;
        }

        index += 1 + usize::from(values.contains(&arg));
    }

    Unwrap { skip: index, ..Unwrap::default() }
}

fn sudo(args: &[&str]) -> Option<Unwrap> {
    let unwrap = options(args, SUDO_VALUES);

    if args[..unwrap.skip.min(args.len())].iter().any(|arg| SUDO_ALONE.contains(arg)) {
        return None;
    }

    Some(unwrap)
}

fn env(args: &[&str]) -> Option<Unwrap> {
    let mut unwrap = Unwrap::default();
    let mut changed = false;

    while let Some(&arg) = args.get(unwrap.skip) {
        if arg == "--" {
            unwrap.skip += 1;
            break;
        }

        if matches!(arg, "-S" | "--split-string") || arg.starts_with("--split-string=") {
            return None;
        }

        if matches!(arg, "-u" | "--unset" | "-C" | "--chdir") {
            changed = true;
            unwrap.skip += 2;
        } else if arg.starts_with('-') {
            changed |= ENV_CHANGES.contains(&arg) || arg.len() > 1;
            unwrap.skip += 1;
        } else if arg.contains('=') {
            unwrap.assigns = true;
            unwrap.skip += 1;
        } else {
            break;
        }
    }

    if changed {
        unwrap.notes.push(Reason::Acts("env".into(), Effect::ChangesEnvironment));
    }

    Some(unwrap)
}

fn timeout(args: &[&str]) -> Unwrap {
    let mut unwrap = options(args, &["-s", "--signal", "-k", "--kill-after"]);

    unwrap.skip += 1;
    unwrap
}

fn time(args: &[&str]) -> Unwrap {
    let mut unwrap = Unwrap::default();

    while let Some(&arg) = args.get(unwrap.skip) {
        if matches!(arg, "-o" | "--output") || arg.starts_with("--output=") {
            unwrap.notes.push(Reason::Acts("time -o".into(), Effect::WritesFile));
        }

        if matches!(arg, "-o" | "--output" | "-f" | "--format") {
            unwrap.skip += 2;
        } else if TIME_FLAGS.contains(&arg)
            || arg.starts_with("--output=")
            || arg.starts_with("--format=")
        {
            unwrap.skip += 1;
        } else {
            break;
        }
    }

    unwrap
}

fn command(args: &[&str]) -> Option<Unwrap> {
    let unwrap = options(args, &[]);

    if args[..unwrap.skip.min(args.len())].iter().any(|arg| matches!(*arg, "-v" | "-V")) {
        return None;
    }

    Some(unwrap)
}
