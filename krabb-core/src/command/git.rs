use super::syntax::long;
use super::{Kind, Verdict};

const GLOBAL: &[&str] =
    &["--no-pager", "-P", "--no-optional-locks", "--literal-pathspecs", "--no-replace-objects"];

const READS: &[&str] = &["status", "rev-parse", "ls-files", "blame"];
const DIFFS: &[&str] = &["log", "show", "diff"];
const DELETES: &[&str] = &["clean", "rm"];

const WRITES: &[&str] = &[
    "add",
    "am",
    "apply",
    "checkout",
    "cherry-pick",
    "clone",
    "commit",
    "config",
    "fetch",
    "gc",
    "init",
    "merge",
    "mv",
    "notes",
    "prune",
    "pull",
    "push",
    "rebase",
    "reset",
    "restore",
    "revert",
    "stash",
    "submodule",
    "switch",
    "tag",
    "update-index",
    "update-ref",
    "worktree",
];

const LISTS: &[&str] = &[
    "--list",
    "--all",
    "--remotes",
    "--contains",
    "--no-contains",
    "--merged",
    "--no-merged",
    "--points-at",
];

const SHOWS: &[&str] = &[
    "--verbose",
    "--show-current",
    "--color",
    "--no-color",
    "--column",
    "--no-column",
    "--ignore-case",
    "--abbrev",
    "--no-abbrev",
    "--sort",
    "--format",
    "--omit-empty",
];

pub(super) fn classify(args: &[&str]) -> Verdict {
    let mut rest = args;

    loop {
        match rest {
            ["-C", _, tail @ ..] => rest = tail,
            [flag, tail @ ..] if GLOBAL.contains(flag) => rest = tail,

            [flag, ..] if flag.starts_with('-') => {
                return Verdict::new(
                    Kind::Unknown,
                    format!("`git {flag}` can change what git runs"),
                );
            }

            _ => break,
        }
    }

    let Some((&command, args)) = rest.split_first() else {
        return Verdict::new(Kind::Unknown, "`git` needs a subcommand");
    };

    if READS.contains(&command) {
        return reads(command);
    }

    if DIFFS.contains(&command) {
        return diff(command, args);
    }

    if command == "branch" {
        return branch(args);
    }

    if DELETES.contains(&command) {
        return Verdict::new(Kind::Deletes, format!("`git {command}` deletes files"));
    }

    if WRITES.contains(&command) {
        return Verdict::new(Kind::Writes, format!("`git {command}` changes the repository"));
    }

    Verdict::new(Kind::Unknown, format!("`git {command}` is not known to be read-only"))
}

fn reads(command: &str) -> Verdict {
    Verdict::new(Kind::ReadOnly, format!("`git {command}` only reads"))
}

fn diff(command: &str, args: &[&str]) -> Verdict {
    for arg in args.iter().take_while(|arg| **arg != "--") {
        if long(arg, "output") {
            return Verdict::new(Kind::Writes, format!("`git {command} --output` writes a file"));
        }

        if long(arg, "ext-diff") {
            return Verdict::new(Kind::Unknown, "`--ext-diff` runs an external program");
        }
    }

    reads(command)
}

fn branch(args: &[&str]) -> Verdict {
    let mut listing = false;
    let mut names = false;

    for arg in args {
        if let Some(option) = arg.strip_prefix("--") {
            let option = format!("--{}", option.split_once('=').map_or(option, |(name, _)| name));

            if option == "--delete" {
                return Verdict::new(Kind::Deletes, "`git branch --delete` deletes branches");
            } else if LISTS.contains(&option.as_str()) {
                listing = true;
            } else if !SHOWS.contains(&option.as_str()) {
                return Verdict::new(Kind::Writes, format!("`git branch {arg}` changes branches"));
            }
        } else if let Some(flags) = arg.strip_prefix('-') {
            if flags.contains(['d', 'D']) {
                return Verdict::new(Kind::Deletes, "`git branch -d` deletes branches");
            }

            if flags.is_empty() || !flags.chars().all(|flag| "alrvi".contains(flag)) {
                return Verdict::new(Kind::Writes, format!("`git branch {arg}` changes branches"));
            }

            listing |= flags.contains(['a', 'l', 'r']);
        } else {
            names = true;
        }
    }

    if names && !listing {
        return Verdict::new(Kind::Writes, "`git branch <name>` creates a branch");
    }

    reads("branch")
}
