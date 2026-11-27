//! Reads what a shell command does from its words, without running it.
//!
//! The reading covers POSIX shells, cmd, and PowerShell. It is a best effort for the
//! approval's fact line: the exact command is always shown below it, so a command this reading
//! misses still gets reviewed in full.

mod segments;

use super::facts::{list, sentence, visible};
use segments::segments;

const DEPTH: usize = 3;
const SEGMENTS: usize = 64;

const SYSTEM_PATHS: [&str; 12] = [
    "/etc", "/usr", "/bin", "/sbin", "/lib", "/lib64", "/boot", "/sys", "/opt", "/private",
    "/Library", "/System",
];

const WINDOWS_PATHS: [&str; 4] =
    ["/windows", "/program files", "/program files (x86)", "/programdata"];

const WINDOWS_VARIABLES: [&str; 9] = [
    "%systemroot%",
    "%windir%",
    "%programfiles%",
    "%programfiles(x86)%",
    "%programdata%",
    "$env:systemroot",
    "$env:windir",
    "$env:programfiles",
    "$env:programdata",
];

const DISCARDED: [&str; 4] = ["nul", "$null", "con", "/dev/null"];

const WRAPPERS: [&str; 18] = [
    "env", "nohup", "command", "exec", "nice", "stdbuf", "time", "then", "do", "else", "elif",
    "if", "while", "until", "!", "{", "}", "$",
];

const DELETES: [&str; 12] = [
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

const WRITES_LAST: [&str; 15] = [
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
];

const WRITES_FIRST: [&str; 7] =
    ["set-content", "add-content", "ac", "out-file", "new-item", "ni", "tee-object"];

const WRITES_ALL: [&str; 5] = ["mkdir", "md", "touch", "tee", "truncate"];
const PERMISSIONS: [&str; 3] = ["chmod", "chown", "chgrp"];

const NETWORK: [&str; 13] = [
    "dig",
    "nslookup",
    "host",
    "aria2c",
    "http",
    "https",
    "invoke-webrequest",
    "iwr",
    "invoke-restmethod",
    "irm",
    "start-bitstransfer",
    "test-netconnection",
    "bitsadmin",
];

const HOSTS: [&str; 7] = ["ping", "nc", "ncat", "netcat", "telnet", "ftp", "whois"];

const ADMIN: [&str; 30] = [
    "service",
    "mount",
    "umount",
    "useradd",
    "userdel",
    "usermod",
    "groupadd",
    "passwd",
    "chpasswd",
    "visudo",
    "modprobe",
    "insmod",
    "rmmod",
    "reboot",
    "shutdown",
    "poweroff",
    "halt",
    "runas",
    "netsh",
    "bcdedit",
    "diskpart",
    "dism",
    "sfc",
    "takeown",
    "set-executionpolicy",
    "new-service",
    "set-service",
    "start-service",
    "stop-service",
    "restart-service",
];

const SERVICE_ACTIONS: [&str; 12] = [
    "start",
    "stop",
    "restart",
    "reload",
    "enable",
    "disable",
    "mask",
    "unmask",
    "daemon-reload",
    "kill",
    "edit",
    "set-property",
];

const SHELLS: [&str; 6] = ["sh", "bash", "zsh", "dash", "ksh", "fish"];
const POWERSHELLS: [&str; 2] = ["powershell", "pwsh"];
const OPAQUE: [&str; 3] = ["invoke-expression", "iex", "eval"];

const SYSTEM_MANAGERS: [&str; 13] = [
    "apt", "apt-get", "aptitude", "dnf", "yum", "zypper", "pacman", "apk", "snap", "port", "dpkg",
    "rpm", "choco",
];

const USER_MANAGERS: [&str; 17] = [
    "brew",
    "pip",
    "pip3",
    "pipx",
    "uv pip",
    "uv",
    "npm",
    "pnpm",
    "yarn",
    "bun",
    "cargo",
    "gem",
    "go",
    "flatpak",
    "winget",
    "scoop",
    "PowerShell",
];

const INSTALLS: [&str; 5] = ["install", "add", "i", "get", "reinstall"];
const REMOVALS: [&str; 7] = ["remove", "uninstall", "purge", "erase", "del", "rm", "autoremove"];
const UPDATES: [&str; 6] = ["update", "upgrade", "dist-upgrade", "full-upgrade", "up", "refresh"];

const MODULES: [(&str, &str); 6] = [
    ("install-module", "install"),
    ("install-package", "install"),
    ("install-script", "install"),
    ("uninstall-module", "uninstall"),
    ("uninstall-package", "uninstall"),
    ("update-module", "update"),
];

const PACKAGE_VALUES: [&str; 14] = [
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
    "-scope",
    "-repository",
    "-requiredversion",
];

#[derive(Default)]
struct Reading {
    packages: Option<Packages>,
    deletes: Vec<String>,
    deletes_unnamed: bool,
    recursive: bool,
    writes: Vec<String>,
    writes_unnamed: bool,
    hosts: Vec<String>,
    network: bool,
    admin: bool,
    opaque: bool,
}

struct Packages {
    action: Action,
    manager: String,
    names: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Install,
    Remove,
    Update,
}

/// The fact line for COMMAND: what kind of action it is, its targets, and its flags.
pub(crate) fn fact(command: &str) -> String {
    let mut reading = Reading::default();

    read(command, &mut reading, 0);

    let kind = if let Some(packages) = &reading.packages {
        let verb = match packages.action {
            Action::Install => "Install",
            Action::Remove => "Remove",
            Action::Update => "Update",
        };

        let names = if packages.names.is_empty() {
            String::new()
        } else {
            format!(": {}", list(&packages.names))
        };

        format!("{verb} packages with {}{names}", visible(&packages.manager))
    } else if !reading.deletes.is_empty() || reading.deletes_unnamed {
        let what = if reading.recursive { "files and folders" } else { "files" };

        if reading.deletes.is_empty() {
            format!("Delete {what}")
        } else {
            format!("Delete {what}: {}", list(&reading.deletes))
        }
    } else {
        "Run a shell command".into()
    };

    sentence(&kind, &flags(&reading))
}

fn flags(reading: &Reading) -> Vec<String> {
    let mut flags = Vec::new();

    if !reading.writes.is_empty() {
        flags.push(format!("writes {}", list(&reading.writes)));
    } else if reading.writes_unnamed {
        flags.push("writes files".into());
    }

    if reading.packages.is_some() && !reading.deletes.is_empty() {
        flags.push(format!("deletes {}", list(&reading.deletes)));
    } else if reading.packages.is_some() && reading.deletes_unnamed {
        flags.push("deletes files".into());
    }

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
    }

    flags
}

fn read(command: &str, reading: &mut Reading, depth: usize) {
    if depth >= DEPTH {
        return;
    }

    for segment in segments(command).into_iter().take(SEGMENTS) {
        for output in segment.outputs {
            if !discarded(&output) {
                wrote(reading, output);
            }
        }

        words(&segment.words, reading, depth);
    }
}

fn words(words: &[String], reading: &mut Reading, depth: usize) {
    let mut index = 0;

    while let Some(word) = words.get(index) {
        let name = program(word);

        if word.contains('=') && !word.starts_with(['-', '=', '$']) {
            index += 1;
            continue;
        }

        match name.as_str() {
            "sudo" | "doas" | "pkexec" | "gsudo" => {
                reading.admin = true;
                index = options(words, index + 1, &["-u", "-g", "-C", "-D", "-h", "-p", "-U"]);
            }

            "su" => {
                reading.admin = true;
                return;
            }

            "timeout" => index = options(words, index + 1, &["-s", "-k"]) + 1,
            "for" | "case" | "function" | "foreach" | "foreach-object" | "%" => return,
            name if WRAPPERS.contains(&name) => index = options(words, index + 1, &["-n", "-u"]),
            _ => break,
        }
    }

    let Some(word) = words.get(index) else {
        return;
    };

    let name = program(word);
    let args = &words[index + 1..];

    for arg in args {
        if let Some(host) = url_host(arg) {
            network(reading, host);
        }

        if let Some(host) = share_host(arg) {
            network(reading, host);
        }
    }

    if packages(&name, args, reading) {
        return;
    }

    command(&name, args, reading, depth);
}

fn command(name: &str, args: &[String], reading: &mut Reading, depth: usize) {
    let windows = windows_program(name);

    match name {
        name if DELETES.contains(&name) => {
            reading.recursive |= matches!(name, "rmdir" | "rd")
                || args.iter().any(|arg| {
                    arg == "--recursive"
                        || arg.eq_ignore_ascii_case("/s")
                        || arg.starts_with('-')
                            && !arg.starts_with("--")
                            && arg.contains(['r', 'R'])
                });

            let targets = operands(args, windows);

            reading.deletes_unnamed |= targets.is_empty();

            for target in targets {
                deleted(reading, target);
            }
        }

        name if WRITES_LAST.contains(&name) => {
            if let Some(target) = operands(args, windows).pop() {
                wrote(reading, target);
            }
        }

        name if WRITES_FIRST.contains(&name) => {
            if let Some(target) = operands(args, windows).into_iter().next() {
                wrote(reading, target);
            } else {
                reading.writes_unnamed = true;
            }
        }

        name if WRITES_ALL.contains(&name) => {
            for target in operands(args, windows) {
                wrote(reading, target);
            }
        }

        name if PERMISSIONS.contains(&name) => {
            for target in operands(args, false).into_iter().skip(1) {
                wrote(reading, target);
            }
        }

        "icacls" => {
            if let Some(target) = operands(args, true).into_iter().next() {
                wrote(reading, target);
            }
        }

        "robocopy" => {
            if let Some(target) = operands(args, true).into_iter().nth(1) {
                wrote(reading, target);
            }
        }

        "sed" | "perl" if args.iter().any(|arg| arg.starts_with("-i")) => {
            for target in operands(args, false).into_iter().skip(1) {
                wrote(reading, target);
            }
        }

        "dd" => {
            for arg in args {
                if let Some(target) = arg.strip_prefix("of=") {
                    wrote(reading, target.into());
                }
            }
        }

        "curl" | "wget" => {
            reading.network = true;
            reading.writes_unnamed |= name == "wget";

            let output: &[&str] =
                if name == "curl" { &["-o", "--output"] } else { &["-O", "--output-document"] };

            for pair in args.windows(2) {
                if output.contains(&pair[0].as_str()) {
                    wrote(reading, pair[1].clone());
                }
            }

            if name == "curl"
                && args.iter().any(|arg| matches!(arg.as_str(), "-O" | "--remote-name"))
            {
                reading.writes_unnamed = true;
            }
        }

        name if NETWORK.contains(&name) => {
            reading.network = true;

            for pair in args.windows(2) {
                if pair[0].eq_ignore_ascii_case("-outfile") {
                    wrote(reading, pair[1].clone());
                }
            }
        }

        "ssh" | "sftp" => {
            let at = options(args, 0, &["-p", "-P", "-i", "-l", "-o", "-F", "-J", "-b"]);

            if let Some(host) = args.get(at).and_then(|arg| remote_host(arg, true)) {
                network(reading, host);
            }

            reading.network = true;
        }

        "scp" | "rsync" => {
            let operands = operands(args, false);

            for arg in &operands {
                if let Some(host) = remote_host(arg, false) {
                    network(reading, host);
                }
            }

            if let Some(target) = operands.last().filter(|arg| remote_host(arg, false).is_none()) {
                wrote(reading, target.clone());
            }

            reading.network |= name == "scp";
        }

        name if HOSTS.contains(&name) => {
            let at =
                options(args, 0, &["-c", "-i", "-W", "-w", "-p", "-s", "-t", "-I", "-q", "-n"]);

            if let Some(host) = args.get(at) {
                network(reading, host);
            }

            reading.network = true;
        }

        "git" => git(args, reading),

        "docker" | "podman" => {
            let action = operands(args, false).into_iter().next();

            reading.network |=
                action.is_some_and(|action| matches!(action.as_str(), "pull" | "push" | "login"));
        }

        "systemctl" if !args.iter().any(|arg| arg == "--user") => {
            reading.admin |= operands(args, false)
                .first()
                .is_some_and(|action| SERVICE_ACTIONS.contains(&action.as_str()));
        }

        "reg" | "sc" | "net" => {
            let action = operands(args, true).into_iter().next().unwrap_or_default();

            reading.admin |= !matches!(
                action.to_ascii_lowercase().as_str(),
                "query" | "queryex" | "qc" | "view" | "use" | "help" | ""
            );
        }

        "start-process" => {
            reading.admin |= args.windows(2).any(|pair| {
                pair[0].eq_ignore_ascii_case("-verb") && pair[1].eq_ignore_ascii_case("runas")
            });
        }

        name if ADMIN.contains(&name) => reading.admin = true,

        name if SHELLS.contains(&name) => {
            if let Some(inner) =
                args.iter().position(|arg| arg == "-c").and_then(|at| args.get(at + 1))
            {
                read(inner, reading, depth + 1);
            }
        }

        "cmd" => {
            let at = args
                .iter()
                .position(|arg| arg.eq_ignore_ascii_case("/c") || arg.eq_ignore_ascii_case("/k"));

            if let Some(at) = at {
                read(&args[at + 1..].join(" "), reading, depth + 1);
            }
        }

        name if POWERSHELLS.contains(&name) => powershell(args, reading, depth),

        name if OPAQUE.contains(&name) => {
            reading.opaque = true;

            if name == "eval" {
                read(&args.join(" "), reading, depth + 1);
            }
        }

        "xargs" => {
            let start = options(args, 0, &["-I", "-n", "-P", "-L", "-d", "-a", "-s", "-E"]);
            nested(&args[start.min(args.len())..], reading, depth);
        }

        "find" => find(args, reading, depth),

        _ => {}
    }
}

fn powershell(args: &[String], reading: &mut Reading, depth: usize) {
    let lowered = args.iter().map(|arg| arg.to_ascii_lowercase()).collect::<Vec<_>>();

    if lowered.iter().any(|arg| matches!(arg.as_str(), "-encodedcommand" | "-enc" | "-e" | "-ec")) {
        reading.opaque = true;
    }

    if let Some(at) = lowered.iter().position(|arg| matches!(arg.as_str(), "-command" | "-c")) {
        read(&args[at + 1..].join(" "), reading, depth + 1);
    }
}

fn find(args: &[String], reading: &mut Reading, depth: usize) {
    if args.iter().any(|arg| arg == "-delete") {
        reading.recursive = true;

        for target in args.iter().take_while(|arg| !arg.starts_with(['-', '(', '!'])) {
            deleted(reading, target.clone());
        }
    }

    for (at, arg) in args.iter().enumerate() {
        if matches!(arg.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir") {
            let end = args[at + 1..]
                .iter()
                .position(|arg| matches!(arg.as_str(), ";" | "+"))
                .map_or(args.len(), |end| at + 1 + end);

            nested(&args[at + 1..end], reading, depth);
        }
    }
}

fn nested(command: &[String], reading: &mut Reading, depth: usize) {
    if depth + 1 < DEPTH {
        words(command, reading, depth + 1);
    }
}

fn git(args: &[String], reading: &mut Reading) {
    let start = options(args, 0, &["-C", "-c", "--git-dir", "--work-tree"]);

    match args.get(start).map(String::as_str) {
        Some("clone" | "fetch" | "pull" | "push" | "ls-remote" | "submodule") => {
            reading.network = true;

            for arg in &args[start + 1..] {
                if let Some(host) = remote_host(arg, false) {
                    network(reading, host);
                }
            }
        }

        Some("clean") => {
            reading.recursive = true;
            deleted(reading, "untracked files".into());
        }

        Some("rm") => {
            for target in operands(&args[start + 1..], false) {
                deleted(reading, target);
            }
        }

        _ => {}
    }
}

/// Records a package manager command in READING, returning whether NAME is one.
fn packages(name: &str, args: &[String], reading: &mut Reading) -> bool {
    if name == "msiexec" {
        return installer(args, reading);
    }

    let module = MODULES.iter().find(|(command, _)| *command == name);

    let (manager, action, args) = match (name, args.first().map(String::as_str), module) {
        (_, _, Some((_, action))) => ("PowerShell", Some(*action), args),

        (python, Some("-m"), _) if python.starts_with("python") || python == "py" => {
            match args.get(1).map(String::as_str) {
                Some(module @ ("pip" | "pip3")) => (module, None, &args[2..]),
                _ => return false,
            }
        }

        ("uv", Some("pip"), _) => ("uv pip", None, &args[1..]),
        _ => (name, None, args),
    };

    let system = SYSTEM_MANAGERS.contains(&manager);

    if !system && !USER_MANAGERS.contains(&manager) {
        return false;
    }

    let Some((action, rest)) = package_action(manager, action, args) else {
        return false;
    };

    let mut names = Vec::new();
    let mut skip = false;

    for arg in rest {
        if std::mem::take(&mut skip) {
            continue;
        }

        if PACKAGE_VALUES.contains(&arg.to_ascii_lowercase().as_str()) {
            skip = true;
        } else if !arg.starts_with('-') {
            names.push(arg.clone());
        }
    }

    reading.network |= action != Action::Remove;
    reading.admin |= system;

    if reading.packages.is_none() {
        reading.packages = Some(Packages { action, manager: manager.into(), names });
    }

    true
}

fn package_action<'a>(
    manager: &str,
    action: Option<&str>,
    args: &'a [String],
) -> Option<(Action, &'a [String])> {
    let first = args.first().map(String::as_str);

    match (manager, first) {
        ("pacman", Some(flag))
            if flag.starts_with("-S") && flag.contains('y') && args.len() == 1 =>
        {
            Some((Action::Update, &args[1..]))
        }

        ("pacman", Some(flag))
            if flag.starts_with("-S") && !flag.contains(['s', 'i', 'c', 'g']) =>
        {
            Some((Action::Install, &args[1..]))
        }

        ("pacman", Some(flag)) if flag.starts_with("-R") => Some((Action::Remove, &args[1..])),
        ("pacman", _) => None,

        ("dpkg" | "rpm", Some("-i" | "--install" | "-U" | "-ivh" | "-Uvh")) => {
            Some((Action::Install, &args[1..]))
        }

        ("dpkg" | "rpm", Some("-r" | "-P" | "--remove" | "--purge" | "-e")) => {
            Some((Action::Remove, &args[1..]))
        }

        ("dpkg" | "rpm", _) => None,

        _ => {
            let (word, rest) = match action {
                Some(action) => (action.to_owned(), args),

                None => {
                    let at = options(args, 0, &["-o", "-c", "-t"]);
                    let word = args.get(at)?.to_ascii_lowercase();

                    (word, &args[(at + 1).min(args.len())..])
                }
            };

            let word = word.as_str();

            let action = if INSTALLS.contains(&word) {
                Action::Install
            } else if REMOVALS.contains(&word) {
                Action::Remove
            } else if UPDATES.contains(&word) {
                Action::Update
            } else {
                return None;
            };

            Some((action, rest))
        }
    }
}

fn installer(args: &[String], reading: &mut Reading) -> bool {
    let lowered = args.iter().map(|arg| arg.to_ascii_lowercase()).collect::<Vec<_>>();

    let action = if lowered.iter().any(|arg| matches!(arg.as_str(), "/i" | "/package")) {
        Action::Install
    } else if lowered.iter().any(|arg| matches!(arg.as_str(), "/x" | "/uninstall")) {
        Action::Remove
    } else {
        return false;
    };

    let names = operands(args, true);

    reading.admin = true;
    reading.packages.get_or_insert(Packages { action, manager: "msiexec".into(), names });
    true
}

/// The index of the first word in WORDS, from START, that is neither a flag nor the value of
/// one of the flags in VALUED.
fn options(words: &[String], start: usize, valued: &[&str]) -> usize {
    let mut index = start;

    while let Some(word) = words.get(index).filter(|word| word.starts_with('-')) {
        index += 1 + usize::from(valued.contains(&word.as_str()));
    }

    index
}

/// The words of ARGS that are not flags. A two-letter `/x` is a cmd switch for any program;

/// for a WINDOWS program, any `/word` without another slash is one too.
fn operands(args: &[String], windows: bool) -> Vec<String> {
    let mut operands = Vec::new();
    let mut ended = false;

    for arg in args {
        if !ended && arg == "--" {
            ended = true;
        } else if ended || !(arg.starts_with('-') || switch(arg, windows)) {
            operands.push(arg.clone());
        }
    }

    operands
}

fn switch(arg: &str, windows: bool) -> bool {
    let Some(rest) = arg.strip_prefix('/') else {
        return false;
    };

    let letters = !rest.is_empty()
        && rest.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '?' | ':' | '-')
        });

    letters && (rest.len() == 1 || windows && !rest.contains('/'))
}

fn windows_program(name: &str) -> bool {
    matches!(
        name,
        "del" | "erase" | "rd" | "copy" | "move" | "xcopy" | "robocopy" | "md" | "ren" | "msiexec"
    ) || matches!(name, "reg" | "sc" | "net" | "netsh" | "runas" | "icacls" | "takeown" | "cmd")
}

/// The program WORD runs: its file name, in lowercase, without a Windows program extension.
fn program(word: &str) -> String {
    let name = word.rsplit(['/', '\\']).next().unwrap_or(word).to_ascii_lowercase();

    for extension in [".exe", ".cmd", ".bat", ".com", ".ps1"] {
        if let Some(stem) = name.strip_suffix(extension) {
            return stem.into();
        }
    }

    name
}

fn url_host(word: &str) -> Option<&str> {
    let (_, rest) = word.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = host.split(':').next()?;

    (!host.is_empty()).then_some(host)
}

fn share_host(word: &str) -> Option<&str> {
    let host = word.strip_prefix("\\\\")?.split('\\').next()?;

    (!host.is_empty() && host != "?" && host != ".").then_some(host)
}

fn remote_host(word: &str, bare: bool) -> Option<&str> {
    if let Some(host) = url_host(word) {
        return Some(host);
    }

    if word.starts_with(['-', '/', '.', '~', '\\']) || drive(word) {
        return None;
    }

    let (host, _) = word.split_once(':').or(bare.then_some((word, "")))?;
    let host = host.rsplit('@').next()?;

    (!host.is_empty()).then_some(host)
}

fn drive(path: &str) -> bool {
    let bytes = path.as_bytes();

    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn discarded(path: &str) -> bool {
    path.starts_with("/dev/") || DISCARDED.iter().any(|name| path.eq_ignore_ascii_case(name))
}

fn network(reading: &mut Reading, host: &str) {
    reading.network = true;

    if !reading.hosts.iter().any(|known| known == host) {
        reading.hosts.push(host.into());
    }
}

fn wrote(reading: &mut Reading, target: String) {
    reading.admin |= system(&target);

    if !reading.writes.contains(&target) {
        reading.writes.push(target);
    }
}

fn deleted(reading: &mut Reading, target: String) {
    reading.admin |= system(&target);

    if !reading.deletes.contains(&target) {
        reading.deletes.push(target);
    }
}

/// Whether PATH lies in a folder only an administrator can usually change, on any platform.
fn system(path: &str) -> bool {
    let lowered = path.to_ascii_lowercase();

    if WINDOWS_VARIABLES.iter().any(|variable| lowered.starts_with(variable)) {
        return true;
    }

    if drive(path) || path.starts_with('\\') && !path.starts_with("\\\\") {
        let rooted =
            lowered.get(usize::from(drive(path)) * 2..).unwrap_or_default().replace('\\', "/");

        return WINDOWS_PATHS.iter().any(|root| within(&rooted, root));
    }

    SYSTEM_PATHS.iter().any(|root| within(path, root))
}

fn within(path: &str, root: &str) -> bool {
    path == root || path.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'))
}
