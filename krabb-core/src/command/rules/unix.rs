use super::Findings;
use crate::command::lex::Word;
use crate::command::option::{Spec, cluster, long, takes_value, walk};
use crate::command::reason::Effect;

const INERT: &[&str] = &[
    "ls",
    "cat",
    "head",
    "wc",
    "stat",
    "du",
    "df",
    "od",
    "hexdump",
    "pwd",
    "echo",
    "which",
    "uname",
    "grep",
    "egrep",
    "fgrep",
    "cut",
    "tr",
    "column",
    "cmp",
    "md5sum",
    "sha1sum",
    "sha224sum",
    "sha256sum",
    "sha384sum",
    "sha512sum",
    "b2sum",
    "cksum",
    "basename",
    "dirname",
    "true",
    "false",
];

const DELETES: &[&str] = &["rm", "rmdir", "unlink", "shred", "srm", "trash", "trash-put"];

const WRITES: &[&str] = &[
    "tee", "mv", "cp", "touch", "mkdir", "chmod", "chown", "chgrp", "ln", "dd", "truncate",
    "install", "mkfifo", "mknod", "patch",
];

const FIND_RUNS: &[&str] = &["-exec", "-execdir", "-ok", "-okdir"];
const FIND_WRITES: &[&str] = &["-fprint", "-fprint0", "-fprintf", "-fls"];

const XXD_VALUES: &[&str] =
    &["-c", "-cols", "-g", "-groupsize", "-l", "-len", "-s", "-seek", "-o", "-offset", "-n"];

const SORT_LONG_VALUES: &[&str] = &[
    "key",
    "field-separator",
    "buffer-size",
    "temporary-directory",
    "random-source",
    "files0-from",
    "batch-size",
];

const DATE_LONG: &[&str] = &[
    "date",
    "reference",
    "file",
    "utc",
    "universal",
    "iso-8601",
    "rfc-email",
    "rfc-3339",
    "debug",
    "resolution",
];

const XARGS: Spec<'static> = Spec {
    flags: "0rtx",
    values: "nLIdPsEa",
    long_flags: &["null", "no-run-if-empty", "verbose", "exit", "replace", "eof"],
    long_values: &[
        "max-args",
        "max-lines",
        "delimiter",
        "max-procs",
        "max-chars",
        "arg-file",
        "process-slot-var",
    ],
};

pub(super) fn deletes(name: &str) -> bool {
    DELETES.contains(&name)
}

pub(super) fn inert(name: &str) -> bool {
    INERT.contains(&name)
}

pub(super) fn evaluate(name: &str, args: &[&str], found: &mut Findings) -> bool {
    let effect = match name {
        _ if inert(name) => Effect::Reads,
        _ if DELETES.contains(&name) => Effect::DeletesFiles,
        _ if WRITES.contains(&name) => Effect::ChangesFiles,
        "find" => return find(args, found),

        "file"
            if args.iter().any(|arg| {
                long(arg, "compile") || cluster(arg).is_some_and(|f| f.contains('C'))
            }) =>
        {
            return flagged(found, "file -C", Effect::WritesFile);
        }

        "file" => Effect::Reads,
        "xxd" if xxd_operands(args) > 1 => return flagged(found, "xxd", Effect::WritesFile),
        "xxd" => Effect::Reads,
        "rg" => return ripgrep(args, found),
        "diff" => return diff(args, found),
        "sort" => return sort(args, found),

        "uniq" if operands(args, &["-f", "-s", "-w"]) > 1 => {
            return flagged(found, "uniq", Effect::WritesFile);
        }

        "uniq" => Effect::Reads,

        "printf" if args.first().is_some_and(|arg| arg.starts_with("-v")) => {
            return flagged(found, "printf -v", Effect::SetsVariable);
        }

        "printf" => Effect::Reads,
        "date" => return date(args, found),
        "env" if args.is_empty() => Effect::ShowsEnvironment,
        "printenv" => Effect::ShowsEnvironment,
        "tail" if follows(args) => return flagged(found, "tail -f", Effect::Waits),
        "tail" => Effect::Reads,
        "env" => Effect::ChangesEnvironment,
        "command" if args.iter().any(|arg| matches!(*arg, "-v" | "-V")) => Effect::Reads,
        "xargs" => return xargs(args, found),
        _ => return false,
    };

    found.acts(name, effect);
    true
}

fn flagged(found: &mut Findings, command: &str, effect: Effect) -> bool {
    found.acts(command, effect);
    true
}

fn find(args: &[&str], found: &mut Findings) -> bool {
    found.acts("find", Effect::Reads);

    for arg in args {
        if FIND_RUNS.contains(arg) {
            found.acts(format!("find {arg}"), Effect::RunsPrograms);
        } else if *arg == "-delete" {
            found.acts("find -delete", Effect::DeletesFiles);
        } else if FIND_WRITES.contains(arg) {
            found.acts(format!("find {arg}"), Effect::WritesFile);
        }
    }

    true
}

pub(in crate::command) fn executions(words: &[Word]) -> Vec<Vec<Word>> {
    let mut executions = Vec::new();

    for (at, word) in words.iter().enumerate() {
        if FIND_RUNS.contains(&word.text.as_str()) {
            let end = words[at + 1..]
                .iter()
                .position(|word| matches!(word.text.as_str(), ";" | "+"))
                .map_or(words.len(), |end| at + 1 + end);

            executions.push(words[at + 1..end].to_vec());
        }
    }

    executions
}

fn xxd_operands(args: &[&str]) -> usize {
    let mut operands = 0;
    let mut index = 0;

    while let Some(&arg) = args.get(index) {
        index += 1;

        if operands > 0 || !arg.starts_with('-') || arg == "-" {
            operands += 1;
        } else if XXD_VALUES.contains(&arg) {
            index += 1;
        }
    }

    operands
}

fn operands(args: &[&str], values: &[&str]) -> usize {
    let mut count = 0;
    let mut index = 0;
    let mut options = true;

    while let Some(&arg) = args.get(index) {
        index += 1;

        if options && arg == "--" {
            options = false;
        } else if options && arg.len() > 1 && arg.starts_with('-') {
            index += usize::from(values.contains(&arg));
        } else {
            count += 1;
        }
    }

    count
}

fn ripgrep(args: &[&str], found: &mut Findings) -> bool {
    for arg in args.iter().take_while(|arg| **arg != "--") {
        let name = arg.split_once('=').map_or(*arg, |(name, _)| name);

        if matches!(name, "--pre" | "--hostname-bin") {
            return flagged(found, &format!("rg {name}"), Effect::RunsPrograms);
        }

        let zipped = name == "--search-zip"
            || cluster(arg).is_some_and(|flags| {
                flags.chars().take_while(|c| !"ABCefgmMtTjErd".contains(*c)).any(|c| c == 'z')
            });

        if zipped {
            return flagged(found, "rg -z", Effect::RunsPrograms);
        }
    }

    flagged(found, "rg", Effect::Reads)
}

fn diff(args: &[&str], found: &mut Findings) -> bool {
    for arg in args.iter().take_while(|arg| **arg != "--") {
        let paginates = long(arg, "paginate")
            || cluster(arg).is_some_and(|flags| {
                flags.chars().take_while(|c| !"IxXSFCUDW".contains(*c)).any(|c| c == 'l')
            });

        if paginates {
            return flagged(found, "diff -l", Effect::RunsPrograms);
        }
    }

    flagged(found, "diff", Effect::Reads)
}

fn sort(args: &[&str], found: &mut Findings) -> bool {
    let mut index = 0;

    while let Some(&arg) = args.get(index) {
        index += 1;

        if arg == "--" {
            break;
        }

        if arg.starts_with("--") {
            if long(arg, "output") {
                return flagged(found, "sort --output", Effect::WritesFile);
            }

            if long(arg, "compress-program") {
                return flagged(found, "sort --compress-program", Effect::RunsPrograms);
            }

            index += usize::from(takes_value(arg, &[], SORT_LONG_VALUES));
            continue;
        }

        let Some(flags) = cluster(arg) else {
            continue;
        };

        for (at, flag) in flags.char_indices() {
            if flag == 'o' {
                return flagged(found, "sort -o", Effect::WritesFile);
            }

            if "ktSTy".contains(flag) {
                index += usize::from(at + 1 == flags.len());
                break;
            }
        }
    }

    flagged(found, "sort", Effect::Reads)
}

fn date(args: &[&str], found: &mut Findings) -> bool {
    let mut index = 0;

    while let Some(&arg) = args.get(index) {
        index += 1;

        if arg.starts_with('+') {
            continue;
        }

        if arg.starts_with("-s") || long(arg, "set") {
            return flagged(found, "date", Effect::SetsClock);
        }

        if matches!(arg, "-d" | "-r" | "-f" | "--date" | "--reference" | "--file") {
            index += 1;
            continue;
        }

        let known = if arg.starts_with("--") {
            DATE_LONG.iter().any(|option| long(arg, option))
        } else if arg.starts_with('-') {
            matches!(arg, "-u" | "-R")
                || ["-I", "-d", "-r", "-f"].iter().any(|o| arg.starts_with(o))
        } else {
            return flagged(found, "date", Effect::SetsClock);
        };

        if !known {
            return flagged(found, &format!("date {arg}"), Effect::Unlisted);
        }
    }

    flagged(found, "date", Effect::Reads)
}

pub(in crate::command) fn xargs_command<S: AsRef<str>>(args: &[S]) -> Result<usize, String> {
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();

    walk(&args, &XARGS).map(|walk| walk.end)
}

fn xargs(args: &[&str], found: &mut Findings) -> bool {
    let start = match xargs_command(args) {
        Ok(start) => start,
        Err(option) => {
            for arg in args {
                if let Some(reason) = super::dangerous(arg.rsplit('/').next().unwrap_or(arg)) {
                    found.push(reason);
                }
            }

            return flagged(found, &format!("xargs {option}"), Effect::Unlisted);
        }
    };

    match args.get(start) {
        None => found.acts("xargs", Effect::Reads),
        Some(inner) if inert(inner) => found.acts("xargs", Effect::Reads),
        Some(inner) => found.acts(format!("xargs {inner}"), Effect::ReceivesOptions),
    }

    true
}

fn follows(args: &[&str]) -> bool {
    args.iter().take_while(|arg| **arg != "--").any(|arg| {
        long(arg, "follow")
            || arg.starts_with('+') && arg.ends_with('f')
            || cluster(arg).is_some_and(|flags| {
                flags.chars().take_while(|c| !"cns".contains(*c)).any(|c| matches!(c, 'f' | 'F'))
            })
    })
}
