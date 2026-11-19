//! Official release artifacts resolved to checksummed archive sources.

use std::path::Path;

type Error = Box<dyn std::error::Error + Send + Sync>;

const BASE: &str = "https://github.com/krabblab/krabb/releases/download";
const LATEST: &str = "https://github.com/krabblab/krabb/releases/latest/download";
const SUMS: &str = "SHA256SUMS";
const SUMS_LIMIT: u64 = 1024 * 1024;

pub(super) fn source(id: &str, version: &str) -> Result<String, Error> {
    source_at(id, version, base().as_deref())
}

/// Resolves a release source for this host from an explicit release base.
pub(super) fn source_at(id: &str, version: &str, base: Option<&str>) -> Result<String, Error> {
    source_from(id, version, base, std::env::consts::OS, std::env::consts::ARCH)
}

/// File name of the release archive for `id` on this host.
#[cfg(test)]
pub(super) fn archive(id: &str, version: &str) -> Option<String> {
    target(std::env::consts::OS, std::env::consts::ARCH)
        .map(|(target, extension)| format!("krabb-{id}-v{version}-{target}.{extension}"))
}

/// Splits a release source into its archive URL and SHA-256 digest.
pub(super) fn parts(source: &str) -> Option<(&str, &str)> {
    source.split_once("#sha256=")
}

/// Whether a recorded plugin source is an official release archive for `id`.
pub(super) fn official(id: &str, source: &str) -> bool {
    official_at(id, source, base().as_deref())
}

/// Like [`official`], against an explicit release base; other hosts are never official.
pub(super) fn official_at(id: &str, source: &str, base: Option<&str>) -> bool {
    let url = parts(source).map_or(source, |(url, _)| url);
    let name = url.rsplit('/').next().unwrap_or(url);
    let origin =
        base.map_or_else(|| format!("{BASE}/"), |base| format!("{}/", base.trim_end_matches('/')));

    url.starts_with(&origin)
        && name.starts_with(&format!("krabb-{id}-v"))
        && super::archive_url(url)
}

/// The newest stable release that publishes the host for this target.
pub(super) fn latest() -> Result<String, Error> {
    latest_from(base().as_deref(), std::env::consts::OS, std::env::consts::ARCH)
}

/// Orders release versions; a pre-release sorts before its release.
pub(super) fn compare(left: &str, right: &str) -> std::cmp::Ordering {
    let key = |version: &str| {
        let mut parts = version.splitn(3, '.');
        let major = parts.next().and_then(|part| part.parse::<u64>().ok()).unwrap_or(0);
        let minor = parts.next().and_then(|part| part.parse::<u64>().ok()).unwrap_or(0);
        let rest = parts.next().unwrap_or("");
        let split = rest.find(['.', '-']).unwrap_or(rest.len());
        let (patch, pre) = rest.split_at(split);
        let patch = patch.parse::<u64>().unwrap_or(0);

        (major, minor, patch, pre.is_empty(), pre.trim_start_matches(['.', '-']).to_owned())
    };

    let (left, right) = (key(left), key(right));

    left.0
        .cmp(&right.0)
        .then(left.1.cmp(&right.1))
        .then(left.2.cmp(&right.2))
        .then(left.3.cmp(&right.3))
        .then_with(|| prerelease(&left.4, &right.4))
}

/// Compares pre-release identifiers field by field, numerically when both are numbers.
fn prerelease(left: &str, right: &str) -> std::cmp::Ordering {
    let mut left = left.split('.');
    let mut right = right.split('.');

    loop {
        let order = match (left.next(), right.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,

            (Some(a), Some(b)) => match (a.parse::<u64>(), b.parse::<u64>()) {
                (Ok(a), Ok(b)) => a.cmp(&b),
                (Ok(_), Err(_)) => std::cmp::Ordering::Less,
                (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
                (Err(_), Err(_)) => a.cmp(b),
            },
        };

        if order != std::cmp::Ordering::Equal {
            return order;
        }
    }
}

/// The configured release location override, if any.
pub(super) fn base() -> Option<String> {
    std::env::var("KRABB_RELEASE_BASE").ok().filter(|value| !value.trim().is_empty())
}

fn latest_from(base: Option<&str>, os: &str, arch: &str) -> Result<String, Error> {
    let (target, extension) = target(os, arch)
        .ok_or_else(|| format!("Official releases are not published for {os} ({arch})."))?;

    let base = base.unwrap_or(LATEST).trim_end_matches('/');

    if !base.starts_with("https://") && !base.starts_with("file://") {
        return Err("KRABB_RELEASE_BASE must use https:// or file://.".into());
    }

    let sums = checksums(base).map_err(|error| {
        format!("The latest release is unavailable: {}", super::sentence(error.to_string()))
    })?;

    let suffix = format!("-{target}.{extension}");

    sums.lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .filter_map(|name| name.trim_start_matches('*').strip_prefix("krabb-core-v"))
        .filter_map(|rest| rest.strip_suffix(&suffix))
        .filter(|version| valid_version(version))
        .max_by(|left, right| compare(left, right))
        .map(str::to_owned)
        .ok_or_else(|| format!("The latest release has no Krabb build for {target}.").into())
}

pub(super) fn valid_version(version: &str) -> bool {
    let digits = |value: &str| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit());
    let mut parts = version.splitn(3, '.');

    let (Some(major), Some(minor), Some(rest)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };

    let split = rest.find(['.', '-']).unwrap_or(rest.len());
    let (patch, suffix) = rest.split_at(split);

    digits(major)
        && digits(minor)
        && digits(patch)
        && (suffix.is_empty()
            || suffix.len() > 1
                && suffix.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'))
}

fn source_from(
    id: &str,
    version: &str,
    base: Option<&str>,
    os: &str,
    arch: &str,
) -> Result<String, Error> {
    if !valid_version(version) {
        return Err(format!("Version {version} is not a valid release version.").into());
    }

    let (target, extension) = target(os, arch).ok_or_else(|| {
        format!(
            "Official releases are not published for {os} ({arch}); \
             install from a local build with --source."
        )
    })?;

    let base = base.map_or_else(|| format!("{BASE}/v{version}"), str::to_owned);
    let base = base.trim_end_matches('/');

    if !base.starts_with("https://") && !base.starts_with("file://") {
        return Err("KRABB_RELEASE_BASE must use https:// or file://.".into());
    }

    let archive = format!("krabb-{id}-v{version}-{target}.{extension}");

    let sums = checksums(base).map_err(|error| {
        format!(
            "Release v{version} is unavailable: {} Check the version or install with --source.",
            super::sentence(error.to_string())
        )
    })?;

    let digest = checksum(&sums, &archive).ok_or_else(|| {
        format!("Release v{version} has no {id} plugin for {target}; check the plugin ID.")
    })?;

    Ok(format!("{base}/{archive}#sha256={digest}"))
}

fn target(os: &str, arch: &str) -> Option<(String, &'static str)> {
    let arch = match arch {
        "x86_64" | "aarch64" => arch,
        _ => return None,
    };

    let (system, extension) = match os {
        "linux" => ("unknown-linux-gnu", "tar.gz"),
        "macos" => ("apple-darwin", "tar.gz"),
        "windows" => ("pc-windows-msvc", "zip"),
        _ => return None,
    };

    Some((format!("{arch}-{system}"), extension))
}

fn checksums(base: &str) -> Result<String, Error> {
    if let Some(path) = base.strip_prefix("file://") {
        return read(&Path::new(path).join(SUMS));
    }

    let temp = super::temp("krabb-release")?;
    let path = temp.join(SUMS);

    let result = super::download(&format!("{base}/{SUMS}"), &path).and_then(|()| read(&path));

    let _ = std::fs::remove_dir_all(&temp);

    result
}

fn read(path: &Path) -> Result<String, Error> {
    let bytes = super::read_bounded(path, SUMS_LIMIT)
        .map_err(|_| "The release checksum list is unreadable or exceeds the size limit.")?;

    Ok(String::from_utf8(bytes)?)
}

fn checksum(sums: &str, archive: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let digest = fields.next()?;
        let name = fields.next()?;

        (name.trim_start_matches('*') == archive
            && digest.len() == 64
            && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| digest.to_ascii_lowercase())
    })
}

#[cfg(test)]
mod tests;
