//! The files a parsed command writes or deletes, read from its program and operands.

use super::parse::Segment;

const DELETES: &[&str] = &[
    "rm",
    "rmdir",
    "rd",
    "del",
    "erase",
    "unlink",
    "shred",
    "srm",
    "trash",
    "trash-put",
    "remove-item",
    "ri",
];

const WRITES_LAST: &[&str] = &[
    "cp",
    "mv",
    "ln",
    "install",
    "copy",
    "move",
    "xcopy",
    "copy-item",
    "cpi",
    "move-item",
    "mi",
    "ren",
    "rename",
    "rename-item",
    "rni",
    "scp",
    "rsync",
];

const WRITES_FIRST: &[&str] =
    &["set-content", "add-content", "ac", "out-file", "new-item", "ni", "tee-object", "icacls"];

const WRITES_ALL: &[&str] = &["mkdir", "md", "touch", "tee", "truncate"];
const PERMISSIONS: &[&str] = &["chmod", "chown", "chgrp"];
const STREAMS: &[&str] = &["/dev/stdout", "/dev/stderr", "/dev/tty"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Targets {
    pub writes: Vec<String>,
    pub deletes: Vec<String>,
    pub recursive: bool,
    pub unnamed_writes: bool,
    pub unnamed_deletes: bool,
    /// Whether the command removes the files Git does not track, such as `git clean`.
    pub untracked: bool,
}

/// What SEGMENT writes and deletes: its output redirections and the operands its program
/// changes. A program that changes files it does not name reports them as unnamed.
pub fn targets(segment: &Segment) -> Targets {
    let mut targets = Targets::default();
    let name = segment.program.as_str();
    let args = segment.args.as_slice();
    let operands = segment.operands();

    for output in &segment.outputs {
        if !STREAMS.contains(&output.as_str()) && !output.starts_with("/dev/fd/") {
            targets.write(output);
        }
    }

    if DELETES.contains(&name) {
        targets.recursive |=
            matches!(name, "rmdir" | "rd") || args.iter().any(|arg| recursive(arg));

        targets.unnamed_deletes |= operands.is_empty();

        for operand in operands {
            targets.delete(operand);
        }
    } else if WRITES_LAST.contains(&name) {
        let directory = args.iter().enumerate().find_map(|(at, arg)| {
            arg.strip_prefix("--target-directory=")
                .or_else(|| (arg == "-t").then(|| args.get(at + 1).map(String::as_str)).flatten())
        });

        let last = operands.last().copied();
        let local = directory.or(last).filter(|operand| super::path::remote(operand).is_none());

        if let Some(operand) = local {
            targets.write(operand);
        }

        if matches!(
            name,
            "mv" | "move" | "move-item" | "mi" | "ren" | "rename" | "rename-item" | "rni"
        ) {
            let sources = operands.iter().filter(|operand| Some(**operand) != local);

            for source in sources {
                targets.write(source);
            }
        }
    } else if WRITES_FIRST.contains(&name) {
        match operands.first() {
            Some(operand) => targets.write(operand),
            None => targets.unnamed_writes = true,
        }
    } else if WRITES_ALL.contains(&name) {
        for operand in operands {
            targets.write(operand);
        }
    } else if PERMISSIONS.contains(&name) {
        for operand in operands.into_iter().skip(1) {
            targets.write(operand);
        }
    } else {
        program(segment, &mut targets);
    }

    targets
}

fn program(segment: &Segment, targets: &mut Targets) {
    let args = segment.args.as_slice();
    let operands = segment.operands();

    match segment.program.as_str() {
        "robocopy" => {
            if let Some(operand) = operands.get(1) {
                targets.write(operand);
            }
        }

        "sed" | "perl" if args.iter().any(|arg| arg.starts_with("-i")) => {
            for operand in operands.into_iter().skip(1) {
                targets.write(operand);
            }
        }

        "dd" => {
            for target in args.iter().filter_map(|arg| arg.strip_prefix("of=")) {
                targets.write(target);
            }
        }

        "curl" => {
            values(args, &["-o", "--output"], false, targets);

            targets.unnamed_writes |=
                args.iter().any(|arg| matches!(arg.as_str(), "-O" | "--remote-name"));
        }

        "wget" => {
            values(args, &["-O", "--output-document"], false, targets);
            targets.unnamed_writes |= targets.writes.is_empty();
        }

        "invoke-webrequest" | "iwr" | "invoke-restmethod" | "irm" => {
            values(args, &["-outfile"], true, targets);
        }

        "find" if args.iter().any(|arg| arg == "-delete") => {
            targets.recursive = true;

            for path in args.iter().take_while(|arg| !arg.starts_with(['-', '(', '!'])) {
                targets.delete(path);
            }
        }

        "git" => git(segment, targets),

        _ => {}
    }
}

fn git(segment: &Segment, targets: &mut Targets) {
    let operands = segment.operands();

    match operands.first().copied() {
        Some("clean") => {
            targets.recursive = true;
            targets.untracked = true;
        }

        Some("rm") => {
            for operand in operands.into_iter().skip(1) {
                targets.delete(operand);
            }
        }

        _ => {}
    }
}

fn values(args: &[String], options: &[&str], caseless: bool, targets: &mut Targets) {
    for pair in args.windows(2) {
        let matches = |option: &&str| match caseless {
            true => pair[0].eq_ignore_ascii_case(option),
            false => pair[0] == *option,
        };

        if options.iter().any(matches) {
            targets.write(&pair[1]);
        }
    }
}

fn recursive(arg: &str) -> bool {
    arg == "--recursive"
        || arg.eq_ignore_ascii_case("/s")
        || arg.starts_with('-') && !arg.starts_with("--") && arg.contains(['r', 'R'])
}

impl Targets {
    fn write(&mut self, target: &str) {
        if !self.writes.iter().any(|known| known == target) {
            self.writes.push(target.into());
        }
    }

    fn delete(&mut self, target: &str) {
        if !self.deletes.iter().any(|known| known == target) {
            self.deletes.push(target.into());
        }
    }
}
