#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
}

const HARMLESS_DEVICES: &[&str] = &["/dev/null", "/dev/stdin", "/dev/zero"];

const LINUX: &[&str] = &[
    "/etc", "/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/boot", "/sys", "/proc", "/opt",
    "/root",
];

const MACOS: &[&str] =
    &["/System", "/Library", "/usr", "/bin", "/sbin", "/private", "/etc", "/var", "/opt"];

const WINDOWS: &[&str] =
    &["\\windows", "\\program files", "\\program files (x86)", "\\programdata"];

const WINDOWS_VARIABLES: &[&str] = &[
    "%systemroot%",
    "%windir%",
    "%programfiles%",
    "%programfiles(x86)%",
    "%programw6432%",
    "%programdata%",
    "$env:systemroot",
    "$env:windir",
    "$env:programfiles",
    "${env:programfiles(x86)}",
    "$env:programdata",
];

pub fn host(word: &str) -> Option<&str> {
    url_host(word).or_else(|| share_host(word))
}

pub fn url_host(word: &str) -> Option<&str> {
    let (scheme, rest) = word.split_once("://")?;

    let scheme_valid = !scheme.is_empty()
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));

    if !scheme_valid {
        return None;
    }

    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = host.split(':').next()?;

    (!host.is_empty()).then_some(host)
}

pub fn share_host(word: &str) -> Option<&str> {
    let path = word.rsplit_once("::").map_or(word, |(_, path)| path);
    let separator = |c: char| c == '\\' || c == '/';
    let mut chars = path.chars();

    if !(chars.next().is_some_and(separator) && chars.next().is_some_and(separator)) {
        return None;
    }

    let rest = &path[2..];
    let mut parts = rest.split(separator);
    let first = parts.next()?;

    let host = match first {
        "?" => {
            let unc = parts.next()?;

            if !unc.eq_ignore_ascii_case("unc") {
                return None;
            }

            parts.next()?
        }

        "." => return None,
        host => host,
    };

    (!host.is_empty()).then_some(host)
}

pub fn device(word: &str) -> bool {
    let windows = word.starts_with("\\\\.\\") || word.starts_with("//./") || namespace(word);
    let parts = components(word);
    let unix = word.starts_with('/') && parts.len() > 1 && parts[0] == "dev";

    windows || unix && !harmless(word)
}

/// Whether WORD's last component is a name Windows reserves for a device in every folder.
pub fn reserved(word: &str) -> bool {
    let name = word.rsplit(['/', '\\']).next().unwrap_or(word);
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches([':', ' ']);
    let stem = stem.to_ascii_lowercase();

    let numbered = |prefix: &str| {
        let mut digits = stem.strip_prefix(prefix).unwrap_or_default().chars();

        matches!(
            (digits.next(), digits.next()),
            (Some(digit), None) if digit.is_ascii_digit() || "¹²³".contains(digit)
        )
    };

    matches!(stem.as_str(), "con" | "prn" | "aux" | "conin$" | "conout$" | "clock$")
        || numbered("com")
        || numbered("lpt")
}

pub fn outside(word: &str, windows: bool) -> bool {
    let separators: &[char] = if windows { &['/', '\\'] } else { &['/'] };

    let parent = word.split(separators).any(|part| part == "..");
    let rooted = word.starts_with(separators) && !harmless(word);
    let bytes = word.as_bytes();
    let drive = windows && bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';

    word.starts_with('~') || parent || rooted || drive
}

fn harmless(word: &str) -> bool {
    let parts = components(word);

    word.starts_with('/') && HARMLESS_DEVICES.iter().any(|device| components(device) == parts)
}

fn components(word: &str) -> Vec<&str> {
    let mut parts = Vec::new();

    for part in word.split('/') {
        match part {
            "" | "." => {}

            ".." => {
                parts.pop();
            }

            part => parts.push(part),
        }
    }

    parts
}

fn namespace(word: &str) -> bool {
    let lowered = word.to_ascii_lowercase().replace('/', "\\");

    if lowered.starts_with("\\??\\") {
        return true;
    }

    let Some(rest) = lowered.strip_prefix("\\\\?\\") else {
        return false;
    };

    let bytes = rest.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';

    !(drive && matches!(bytes.get(2), None | Some(b'\\')) || rest.starts_with("unc\\"))
}

pub fn system(path: &str, os: Os) -> bool {
    match os {
        Os::Linux => LINUX.iter().any(|root| within(path, root, '/')),
        Os::MacOs => MACOS.iter().any(|root| within(path, root, '/')),
        Os::Windows => windows_system(path),
    }
}

fn windows_system(path: &str) -> bool {
    let lowered = path.to_ascii_lowercase().replace('/', "\\");

    if WINDOWS_VARIABLES.iter().any(|variable| lowered.starts_with(variable)) {
        return true;
    }

    let lowered = lowered.strip_prefix("\\\\?\\").unwrap_or(&lowered);
    let bytes = lowered.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';

    let rooted = if drive {
        &lowered[2..]
    } else if lowered.starts_with('\\') && !lowered.starts_with("\\\\") {
        lowered
    } else {
        return false;
    };

    WINDOWS.iter().any(|root| within(rooted, root, '\\'))
}

fn within(path: &str, root: &str, separator: char) -> bool {
    path == root || path.strip_prefix(root).is_some_and(|rest| rest.starts_with(separator))
}
