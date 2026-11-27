use super::{Kind, Verdict, git};

const ELEVATES: &[&str] = &["sudo", "doas", "su", "runas", "pkexec", "gsudo"];

const MANAGERS: &[&str] = &[
    "apt", "apt-get", "aptitude", "dnf", "yum", "pacman", "zypper", "brew", "snap", "flatpak",
    "port", "apk", "emerge", "nix-env", "winget", "choco", "scoop", "msiexec",
];

const INSTALLERS: &[&str] = &["pip", "pip3", "cargo", "gem", "go"];
const NODE: &[&str] = &["npm", "pnpm", "yarn"];

const CODE: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "dash",
    "ksh",
    "fish",
    "csh",
    "tcsh",
    "eval",
    "exec",
    "source",
    ".",
    "python",
    "python3",
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
    "ssh",
    "scp",
    "rsync",
    "invoke-webrequest",
    "iwr",
    "invoke-restmethod",
    "irm",
];

pub(super) fn reads(name: &str) -> Verdict {
    Verdict::new(Kind::ReadOnly, format!("`{name}` only reads"))
}

pub(super) fn unknown(name: &str) -> Verdict {
    Verdict::new(Kind::Unknown, format!("`{name}` is not known to be read-only"))
}

pub(super) fn deletes(name: &str) -> Verdict {
    Verdict::new(Kind::Deletes, format!("`{name}` deletes files"))
}

pub(super) fn writes(name: &str) -> Verdict {
    Verdict::new(Kind::Writes, format!("`{name}` changes files"))
}

pub(super) fn elevates(name: &str) -> Verdict {
    Verdict::new(Kind::Elevates, format!("`{name}` runs with admin rights"))
}

pub(super) fn installs(name: &str) -> Verdict {
    Verdict::new(Kind::Installs, format!("`{name}` installs or removes software"))
}

pub(super) fn shared(name: &str, args: &[&str]) -> Option<Verdict> {
    if name == "git" {
        return Some(git::classify(args));
    }

    if ELEVATES.contains(&name) {
        return Some(elevates(name));
    }

    if MANAGERS.contains(&name) {
        return Some(installs(name));
    }

    if INSTALLERS.contains(&name) {
        let command = args.iter().find(|arg| !arg.starts_with('-'));

        if matches!(command, Some(&"install" | &"uninstall")) {
            return Some(installs(name));
        }

        return Some(unknown(name));
    }

    if NODE.contains(&name) {
        let global = args.iter().any(|arg| matches!(*arg, "-g" | "--global" | "global"));

        return Some(if global {
            installs(name)
        } else {
            Verdict::new(Kind::Unknown, format!("`{name}` can run package scripts"))
        });
    }

    if CODE.contains(&name) {
        return Some(Verdict::new(Kind::Unknown, format!("`{name}` runs arbitrary code")));
    }

    if NETWORK.contains(&name) {
        return Some(Verdict::new(Kind::Unknown, format!("`{name}` uses the network")));
    }

    None
}
