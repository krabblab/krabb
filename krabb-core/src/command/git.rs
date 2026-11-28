use super::option::{cluster, long, takes_value};
use super::reason::Effect;
use super::rules::Findings;

const GLOBAL_VALUES: &[&str] =
    &["-C", "-c", "--git-dir", "--work-tree", "--namespace", "--exec-path"];

const RUNS: &[&str] = &["-c", "--config-env", "--exec-path"];
const DIFFS: &[&str] = &["log", "show", "diff", "whatchanged"];
const DELETES: &[&str] = &["clean", "rm"];
const NETWORK: &[&str] = &["clone", "fetch", "pull", "push", "ls-remote", "submodule", "remote"];

const CONFIG_READS: &[&str] =
    &["--get", "--get-all", "--get-regexp", "--get-urlmatch", "--list", "-l", "get", "list"];

const DIFF_SHORT_VALUES: &[&str] = &["-n", "-S", "-G", "-O", "-L"];

const DIFF_LONG_VALUES: &[&str] = &[
    "author",
    "committer",
    "grep",
    "since",
    "until",
    "after",
    "before",
    "skip",
    "max-count",
    "encoding",
    "date",
    "diff-algorithm",
];

const WRITES: &[&str] = &[
    "add",
    "am",
    "apply",
    "checkout",
    "cherry-pick",
    "commit",
    "gc",
    "init",
    "merge",
    "mv",
    "notes",
    "prune",
    "rebase",
    "reset",
    "restore",
    "revert",
    "stash",
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

pub(super) fn evaluate(args: &[&str], found: &mut Findings) {
    found.acts("git", Effect::RunsConfigured);

    let mut index = 0;

    while let Some(&arg) = args.get(index).filter(|arg| arg.starts_with('-')) {
        let name = arg.split_once('=').map_or(arg, |(name, _)| name);

        if RUNS.contains(&name) {
            found.acts(format!("git {name}"), Effect::RunsPrograms);
        }

        index += 1 + usize::from(GLOBAL_VALUES.contains(&arg));
    }

    let Some(&command) = args.get(index) else {
        return;
    };

    let args = args.get(index + 1..).unwrap_or_default();

    if DIFFS.contains(&command) {
        diff(command, args, found);
    } else if command == "branch" {
        branch(args, found);
    } else if command == "config" {
        if !args.iter().any(|arg| CONFIG_READS.contains(arg)) {
            found.acts("git config", Effect::ChangesConfiguration);
        }
    } else if DELETES.contains(&command) {
        found.acts(format!("git {command}"), Effect::DeletesFiles);
    } else if NETWORK.contains(&command) {
        found.acts(format!("git {command}"), Effect::UsesNetwork);

        if command != "ls-remote" {
            found.acts(format!("git {command}"), Effect::ChangesRepository);
        }
    } else if WRITES.contains(&command) {
        found.acts(format!("git {command}"), Effect::ChangesRepository);
    }
}

fn diff(command: &str, args: &[&str], found: &mut Findings) {
    let mut index = 0;

    while let Some(&arg) = args.get(index) {
        index += 1;

        if arg == "--" {
            break;
        }

        if long(arg, "output") {
            found.acts(format!("git {command} --output"), Effect::WritesFile);
        } else if long(arg, "ext-diff") {
            found.acts(format!("git {command} --ext-diff"), Effect::RunsPrograms);
        }

        index += usize::from(takes_value(arg, DIFF_SHORT_VALUES, DIFF_LONG_VALUES));
    }
}

fn branch(args: &[&str], found: &mut Findings) {
    let mut listing = false;
    let mut names = false;

    for arg in args {
        if let Some(option) = arg.strip_prefix("--") {
            let option = format!("--{}", option.split_once('=').map_or(option, |(name, _)| name));

            if option == "--delete" {
                found.acts("git branch --delete", Effect::DeletesBranches);
                return;
            } else if LISTS.contains(&option.as_str()) {
                listing = true;
            } else if !SHOWS.contains(&option.as_str()) {
                found.acts(format!("git branch {option}"), Effect::ChangesBranches);
                return;
            }
        } else if let Some(flags) = cluster(arg) {
            if flags.contains(['d', 'D']) {
                found.acts("git branch -d", Effect::DeletesBranches);
                return;
            }

            if !flags.chars().all(|flag| "alrvi".contains(flag)) {
                found.acts(format!("git branch {arg}"), Effect::ChangesBranches);
                return;
            }

            listing |= flags.contains(['a', 'l', 'r']);
        } else {
            names = true;
        }
    }

    if names && !listing {
        found.acts("git branch", Effect::CreatesBranch);
    }
}
