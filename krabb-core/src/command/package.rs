#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Install,
    Remove,
    Update,
    Query,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packages {
    pub manager: String,
    pub action: Action,
    pub system: bool,
    pub names: Vec<String>,
}

const SYSTEM: &[&str] = &[
    "apt", "apt-get", "aptitude", "dnf", "yum", "zypper", "pacman", "apk", "snap", "port", "dpkg",
    "rpm", "emerge", "choco", "msiexec",
];

const USER: &[&str] = &[
    "brew",
    "pip",
    "pip3",
    "pipx",
    "uv pip",
    "npm",
    "pnpm",
    "yarn",
    "bun",
    "cargo",
    "gem",
    "go",
    "flatpak",
    "nix-env",
    "winget",
    "scoop",
    "powershell",
];

const INSTALLS: &[&str] = &["install", "add", "i", "in", "get", "reinstall"];
const REMOVALS: &[&str] = &["remove", "uninstall", "purge", "erase", "del", "rm", "autoremove"];
const UPDATES: &[&str] = &["update", "upgrade", "dist-upgrade", "full-upgrade", "up", "refresh"];

const QUERIES: &[&str] = &[
    "list", "search", "info", "show", "query", "outdated", "view", "policy", "madison", "ls", "se",
    "why", "home", "desc", "freeze", "check", "doctor", "deps", "uses",
];

const VALUES: &[&str] = &[
    "-r",
    "--requirement",
    "-c",
    "--constraint",
    "-t",
    "--target",
    "-i",
    "--index-url",
    "-o",
    "--source",
    "--scope",
    "--version",
    "-v",
    "--prefix",
];

const MODULES: &[(&str, Action)] = &[
    ("install-module", Action::Install),
    ("install-package", Action::Install),
    ("install-script", Action::Install),
    ("uninstall-module", Action::Remove),
    ("uninstall-package", Action::Remove),
    ("uninstall-script", Action::Remove),
    ("update-module", Action::Update),
    ("update-script", Action::Update),
    ("find-module", Action::Query),
    ("find-package", Action::Query),
    ("get-package", Action::Query),
    ("get-installedmodule", Action::Query),
];

pub(super) fn manager(name: &str, args: &[&str]) -> bool {
    SYSTEM.contains(&name)
        || USER.contains(&name)
        || MODULES.iter().any(|(module, _)| *module == name)
        || python_pip(name, args).is_some()
}

pub(super) fn read(name: &str, args: &[&str]) -> Option<Packages> {
    if name == "msiexec" {
        return installer(args);
    }

    let module = MODULES.iter().find(|(module, _)| *module == name).map(|(_, action)| *action);

    let (manager, fixed, args) = match (name, args.first(), module) {
        (_, _, Some(action)) => ("powershell", Some(action), args),
        ("uv", Some(&"pip"), _) => ("uv pip", None, &args[1..]),

        _ => match python_pip(name, args) {
            Some((manager, rest)) => (manager, None, rest),
            None => (name, None, args),
        },
    };

    let system = SYSTEM.contains(&manager);

    if !system && !USER.contains(&manager) {
        return None;
    }

    let (action, rest) = match fixed {
        Some(action) => (action, args),
        None => action(manager, args)?,
    };

    Some(Packages { manager: manager.into(), action, system, names: names(rest) })
}

fn python_pip<'a>(name: &str, args: &'a [&'a str]) -> Option<(&'static str, &'a [&'a str])> {
    let python = name.starts_with("python") || name == "py";

    match (python, args) {
        (true, ["-m", "pip" | "pip3", rest @ ..]) => Some(("pip", rest)),
        _ => None,
    }
}

fn action<'a>(manager: &str, args: &'a [&'a str]) -> Option<(Action, &'a [&'a str])> {
    let first = args.first().copied().unwrap_or_default();
    let rest = args.get(1..).unwrap_or_default();

    match manager {
        "pacman" => {
            let action = if first.starts_with("-S") && first.contains('y') && args.len() == 1 {
                Action::Update
            } else if first.starts_with("-Q")
                || first.starts_with("-S") && first.contains(['s', 'i'])
            {
                Action::Query
            } else if first.starts_with("-S") && !first.contains(['c', 'g']) {
                Action::Install
            } else if first.starts_with("-R") {
                Action::Remove
            } else {
                return None;
            };

            Some((action, rest))
        }

        "dpkg" | "rpm" => {
            let action = match first {
                "-i" | "--install" | "-U" | "-ivh" | "-Uvh" => Action::Install,
                "-r" | "-P" | "--remove" | "--purge" | "-e" => Action::Remove,

                "-l" | "-L" | "-s" | "-S" | "-q" | "-qa" | "-qi" | "-ql" | "--list" => {
                    Action::Query
                }

                _ => return None,
            };

            Some((action, rest))
        }

        _ => {
            let at = skip_options(args, &["-o", "-c", "-t"]);
            let word = args.get(at)?.to_ascii_lowercase();
            let rest = args.get(at + 1..).unwrap_or_default();

            let action = if INSTALLS.contains(&word.as_str()) {
                Action::Install
            } else if REMOVALS.contains(&word.as_str()) {
                Action::Remove
            } else if UPDATES.contains(&word.as_str()) {
                Action::Update
            } else if QUERIES.contains(&word.as_str()) {
                Action::Query
            } else {
                return None;
            };

            Some((action, rest))
        }
    }
}

fn installer(args: &[&str]) -> Option<Packages> {
    let has = |options: &[&str]| {
        args.iter().any(|arg| options.iter().any(|o| arg.eq_ignore_ascii_case(o)))
    };

    let action = if has(&["/i", "/package"]) {
        Action::Install
    } else if has(&["/x", "/uninstall"]) {
        Action::Remove
    } else {
        return None;
    };

    let names = args.iter().filter(|arg| !arg.starts_with(['/', '-'])).map(|arg| (*arg).to_owned());

    Some(Packages { manager: "msiexec".into(), action, system: true, names: names.collect() })
}

fn names(args: &[&str]) -> Vec<String> {
    let mut names = Vec::new();
    let mut skip = false;

    for arg in args {
        if std::mem::take(&mut skip) {
            continue;
        }

        if VALUES.contains(&arg.to_ascii_lowercase().as_str()) {
            skip = true;
        } else if !arg.starts_with('-') {
            names.push((*arg).to_owned());
        }
    }

    names
}

fn skip_options(args: &[&str], valued: &[&str]) -> usize {
    let mut index = 0;

    while let Some(arg) = args.get(index).filter(|arg| arg.starts_with('-')) {
        index += 1 + usize::from(valued.contains(arg));
    }

    index
}
