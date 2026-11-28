use super::Dialect;
use super::option::{Spec, Walk, walk};
use super::reason::{Effect, Reason};

const HELP: &[&str] = &["--help", "--version"];

struct Wrapper {
    name: &'static str,
    portable: bool,
    elevates: bool,
    operands: usize,
    spec: Spec<'static>,
    alone: &'static [&'static str],
}

const WRAPPERS: &[Wrapper] = &[
    Wrapper {
        name: "sudo",
        portable: true,
        elevates: true,
        operands: 0,
        spec: Spec {
            flags: "AbEHhiKklnPSsVv",
            values: "ugCDpUrtTR",
            long_flags: &[
                "askpass",
                "background",
                "bell",
                "edit",
                "help",
                "list",
                "login",
                "non-interactive",
                "preserve-env",
                "preserve-groups",
                "remove-timestamp",
                "reset-timestamp",
                "set-home",
                "shell",
                "stdin",
                "validate",
                "version",
            ],
            long_values: &[
                "user",
                "group",
                "close-from",
                "chdir",
                "host",
                "prompt",
                "other-user",
                "role",
                "type",
                "command-timeout",
                "chroot",
            ],
        },
        alone: &[
            "-e",
            "--edit",
            "-l",
            "--list",
            "-v",
            "--validate",
            "-K",
            "--remove-timestamp",
            "-V",
            "--version",
            "-h",
            "--help",
            "--host",
        ],
    },
    Wrapper {
        name: "gsudo",
        portable: true,
        elevates: true,
        operands: 0,
        spec: Spec {
            flags: "nwdsk",
            values: "ui",
            long_flags: &["new", "wait", "direct", "system", "ti", "loadprofile", "help", "version"],
            long_values: &["user", "integrity"],
        },
        alone: HELP,
    },
    Wrapper {
        name: "doas",
        portable: false,
        elevates: true,
        operands: 0,
        spec: Spec { flags: "nsL", values: "uaC", long_flags: &[], long_values: &[] },
        alone: &["-L", "-C"],
    },
    Wrapper {
        name: "pkexec",
        portable: false,
        elevates: true,
        operands: 0,
        spec: Spec {
            flags: "",
            values: "",
            long_flags: &["disable-internal-agent", "keep-cwd", "help", "version"],
            long_values: &["user"],
        },
        alone: HELP,
    },
    Wrapper {
        name: "run0",
        portable: false,
        elevates: true,
        operands: 0,
        spec: Spec {
            flags: "h",
            values: "ugD",
            long_flags: &["no-ask-password", "help", "version"],
            long_values: &[
                "user",
                "group",
                "nice",
                "chdir",
                "setenv",
                "unit",
                "property",
                "description",
                "slice",
                "machine",
                "background",
                "shell-prompt-prefix",
            ],
        },
        alone: &["-h", "--help", "--version"],
    },
    Wrapper {
        name: "env",
        portable: false,
        elevates: false,
        operands: 0,
        spec: Spec {
            flags: "i0v",
            values: "uCS",
            long_flags: &[
                "ignore-environment",
                "null",
                "debug",
                "ignore-signal",
                "default-signal",
                "block-signal",
                "list-signal-handling",
                "help",
                "version",
            ],
            long_values: &["unset", "chdir", "split-string"],
        },
        alone: &["-S", "--split-string", "--help", "--version"],
    },
    Wrapper {
        name: "nice",
        portable: false,
        elevates: false,
        operands: 0,
        spec: Spec {
            flags: "0123456789",
            values: "n",
            long_flags: &["help", "version"],
            long_values: &["adjustment"],
        },
        alone: HELP,
    },
    Wrapper {
        name: "timeout",
        portable: false,
        elevates: false,
        operands: 1,
        spec: Spec {
            flags: "v",
            values: "sk",
            long_flags: &["preserve-status", "foreground", "verbose", "help", "version"],
            long_values: &["signal", "kill-after"],
        },
        alone: HELP,
    },
    Wrapper {
        name: "time",
        portable: false,
        elevates: false,
        operands: 0,
        spec: Spec {
            flags: "pvaq",
            values: "of",
            long_flags: &["portability", "verbose", "append", "quiet", "help", "version"],
            long_values: &["output", "format"],
        },
        alone: HELP,
    },
    Wrapper {
        name: "command",
        portable: false,
        elevates: false,
        operands: 0,
        spec: Spec { flags: "pvV", values: "", long_flags: &[], long_values: &[] },
        alone: &["-v", "-V"],
    },
    Wrapper {
        name: "nohup",
        portable: false,
        elevates: false,
        operands: 0,
        spec: Spec { flags: "", values: "", long_flags: &["help", "version"], long_values: &[] },
        alone: HELP,
    },
    Wrapper {
        name: "stdbuf",
        portable: false,
        elevates: false,
        operands: 0,
        spec: Spec {
            flags: "",
            values: "ioe",
            long_flags: &["help", "version"],
            long_values: &["input", "output", "error"],
        },
        alone: HELP,
    },
    Wrapper {
        name: "ionice",
        portable: false,
        elevates: false,
        operands: 0,
        spec: Spec {
            flags: "t",
            values: "cnpPu",
            long_flags: &["ignore", "help", "version"],
            long_values: &["class", "classdata", "pid", "pgid", "uid"],
        },
        alone: &["-p", "--pid", "-P", "--pgid", "-u", "--uid", "--help", "--version"],
    },
    Wrapper {
        name: "setsid",
        portable: false,
        elevates: false,
        operands: 0,
        spec: Spec {
            flags: "cfw",
            values: "",
            long_flags: &["ctty", "fork", "wait", "help", "version"],
            long_values: &[],
        },
        alone: HELP,
    },
    Wrapper {
        name: "flock",
        portable: false,
        elevates: false,
        operands: 1,
        spec: Spec {
            flags: "sxenuoFv",
            values: "wEc",
            long_flags: &[
                "shared",
                "exclusive",
                "nonblock",
                "nb",
                "close",
                "unlock",
                "no-fork",
                "verbose",
                "help",
                "version",
            ],
            long_values: &["timeout", "conflict-exit-code", "command"],
        },
        alone: &["-c", "--command", "--help", "--version"],
    },
    Wrapper {
        name: "chrt",
        portable: false,
        elevates: false,
        operands: 1,
        spec: Spec {
            flags: "abdfiormRvp",
            values: "TPD",
            long_flags: &[
                "all-tasks",
                "batch",
                "deadline",
                "fifo",
                "idle",
                "other",
                "rr",
                "reset-on-fork",
                "verbose",
                "max",
                "pid",
                "help",
                "version",
            ],
            long_values: &["sched-runtime", "sched-period", "sched-deadline"],
        },
        alone: &["-m", "--max", "-p", "--pid", "--help", "--version"],
    },
    Wrapper {
        name: "taskset",
        portable: false,
        elevates: false,
        operands: 1,
        spec: Spec {
            flags: "acp",
            values: "",
            long_flags: &["all-tasks", "cpu-list", "pid", "help", "version"],
            long_values: &[],
        },
        alone: &["-p", "--pid", "--help", "--version"],
    },
    Wrapper {
        name: "unbuffer",
        portable: false,
        elevates: false,
        operands: 0,
        spec: Spec { flags: "p", values: "", long_flags: &[], long_values: &[] },
        alone: &[],
    },
    Wrapper {
        name: "busybox",
        portable: false,
        elevates: false,
        operands: 0,
        spec: Spec { flags: "", values: "", long_flags: &[], long_values: &[] },
        alone: &[],
    },
];

#[derive(Default)]
pub(super) struct Unwrap {
    pub skip: usize,
    pub elevated: bool,
    pub assigns: bool,
    pub notes: Vec<Reason>,
}

pub(super) enum Outcome {
    Runs(Unwrap),
    Unrecognized { option: String, elevates: bool },
}

pub(super) fn unwrap(dialect: Dialect, name: &str, args: &[&str]) -> Option<Outcome> {
    let wrapper = WRAPPERS
        .iter()
        .find(|wrapper| wrapper.name == name && (wrapper.portable || dialect == Dialect::Sh))?;

    let walk = match walk(args, &wrapper.spec) {
        Ok(walk) => walk,

        Err(option) => {
            return Some(Outcome::Unrecognized { option, elevates: wrapper.elevates });
        }
    };

    if walk.has(wrapper.alone) {
        return None;
    }

    let mut unwrap = Unwrap {
        skip: walk.end + wrapper.operands,
        elevated: wrapper.elevates,
        ..Unwrap::default()
    };

    let trailing = args.get(unwrap.skip).is_some_and(|arg| arg.starts_with('-'));

    if wrapper.operands > 0 && trailing {
        return None;
    }

    notes(name, args, &walk, &mut unwrap);

    (unwrap.skip < args.len()).then_some(Outcome::Runs(unwrap))
}

fn notes(name: &str, args: &[&str], walk: &Walk, unwrap: &mut Unwrap) {
    match name {
        "env" => {
            let dash = args.get(unwrap.skip) == Some(&"-");

            unwrap.skip += usize::from(dash);

            while args.get(unwrap.skip).is_some_and(|arg| arg.contains('=')) {
                unwrap.assigns = true;
                unwrap.skip += 1;
            }

            if dash || !walk.seen.is_empty() {
                unwrap.notes.push(Reason::Acts("env".into(), Effect::ChangesEnvironment));
            }
        }

        "time" if walk.has(&["-o", "--output"]) => {
            unwrap.notes.push(Reason::Acts("time -o".into(), Effect::WritesFile));
        }

        "nohup" => unwrap.notes.push(Reason::Acts("nohup".into(), Effect::WritesFile)),
        "flock" => unwrap.notes.push(Reason::Acts("flock".into(), Effect::WritesFile)),
        _ => {}
    }
}
