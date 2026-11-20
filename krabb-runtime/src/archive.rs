//! Downloads, verifies, and safely unpacks plugin release archives.

use crate::*;

pub(crate) fn archive(path: &Path) -> bool {
    path.file_name().is_some_and(|name| archive_name(&name.to_string_lossy().to_ascii_lowercase()))
}

pub(crate) fn archive_url(value: &str) -> bool {
    archive_name(value.split('?').next().unwrap_or(value))
}

fn archive_name(value: &str) -> bool {
    value.ends_with(".tar")
        || value.ends_with(".tar.gz")
        || value.ends_with(".tgz")
        || value.ends_with(".zip")
}

pub(crate) fn canonical_source(source: &str) -> String {
    let (base, fragment) =
        source.split_once('#').map_or((source, None), |(base, fragment)| (base, Some(fragment)));

    let path = base
        .strip_prefix("file://")
        .map(PathBuf::from)
        .or_else(|| Path::new(base).exists().then(|| PathBuf::from(base)))
        .or_else(|| relocated_plugin_source(Path::new(base)))
        .map(|path| relocated_plugin_source(&path).unwrap_or(path));

    let base = path.and_then(|path| std::fs::canonicalize(path).ok()).map_or_else(
        || base.to_owned(),
        |path| {
            if source.starts_with("file://") {
                format!("file://{}", path.display())
            } else {
                path.display().to_string()
            }
        },
    );

    fragment.map_or(base.clone(), |fragment| format!("{base}#{fragment}"))
}

pub(crate) fn embedded(value: &str) -> bool {
    let (scheme, rest) = if let Some(rest) = value.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = value.strip_prefix("http://") {
        ("http", rest)
    } else if let Some(rest) = value.strip_prefix("ssh://") {
        ("ssh", rest)
    } else {
        return false;
    };

    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);

    let Some((user, _)) = authority.split_once('@') else {
        return false;
    };

    scheme != "ssh" || user.contains(':')
}

pub(crate) fn archive_temp() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    temp("krabb-archive")
}

pub(crate) fn temp(prefix: &str) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    for _ in 0..8 {
        let mut bytes = [0_u8; 16];

        getrandom::fill(&mut bytes)?;

        let suffix = digest::hex(&bytes);
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{suffix}", std::process::id()));

        match std::fs::create_dir(&path) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    let mut permissions = std::fs::metadata(&path)?.permissions();
                    permissions.set_mode(0o700);
                    std::fs::set_permissions(&path, permissions)?;
                }

                return Ok(std::fs::canonicalize(path)?);
            }

            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }

    Err("Could not create a private temporary directory.".into())
}

pub(crate) fn verify_archive(
    path: &Path,
    expected: Option<&str>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if std::fs::metadata(path)?.len() > ARCHIVE_LIMIT {
        return Err("Archive exceeds the size limit.".into());
    }

    let Some(expected) = expected else {
        return Err("Archive sources require a #sha256= checksum.".into());
    };

    let mut hash = Sha256::new();

    hash.update(std::fs::read(path)?);

    let actual = hex_digest(hash.finalize());

    if actual != expected.to_ascii_lowercase() {
        return Err("Archive checksum did not match the requested SHA-256.".into());
    }

    Ok(())
}

pub(crate) fn unpack(
    archive_path: &Path,
    destination: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    validate_archive(archive_path)?;
    archive_bounds(archive_path)?;

    let name = archive_path.to_string_lossy().to_ascii_lowercase();

    let (program, args): (&str, Vec<String>) = if name.ends_with(".zip") && cfg!(windows) {
        (
            "tar",
            vec![
                "-xf".into(),
                archive_path.display().to_string(),
                "-C".into(),
                destination.display().to_string(),
            ],
        )
    } else if name.ends_with(".zip") {
        (
            "unzip",
            vec![
                "-q".into(),
                archive_path.display().to_string(),
                "-d".into(),
                destination.display().to_string(),
            ],
        )
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        (
            "tar",
            vec![
                "-xzf".into(),
                archive_path.display().to_string(),
                "-C".into(),
                destination.display().to_string(),
            ],
        )
    } else {
        (
            "tar",
            vec![
                "-xf".into(),
                archive_path.display().to_string(),
                "-C".into(),
                destination.display().to_string(),
            ],
        )
    };

    let output = command_output(std::process::Command::new(program).args(args))?;

    if !output.status.success() {
        return Err(
            format!("Archive extraction failed: {}", sentence(redact(&output.stderr))).into()
        );
    }

    safe_archive(destination)?;
    Ok(())
}

pub(crate) fn archive_bounds(path: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let name = path.to_string_lossy().to_ascii_lowercase();

    let (program, args): (&str, Vec<String>) = if name.ends_with(".zip") && cfg!(windows) {
        ("tar", vec!["-tvf".into(), path.display().to_string()])
    } else if name.ends_with(".zip") {
        ("unzip", vec!["-l".into(), path.display().to_string()])
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        ("tar", vec!["-tvzf".into(), path.display().to_string()])
    } else {
        ("tar", vec!["-tvf".into(), path.display().to_string()])
    };

    let output = archive_listing(program, &args)?;
    let mut files = 0_usize;
    let mut bytes = 0_u64;

    for line in String::from_utf8_lossy(&output).lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();

        if fields.is_empty() {
            continue;
        }

        let size = if name.ends_with(".zip") && !cfg!(windows) {
            fields.first().and_then(|value| value.parse::<u64>().ok()).filter(|_| fields.len() >= 4)
        } else {
            tar_size(&fields)
        };

        let Some(size) = size else {
            if !name.ends_with(".zip") || cfg!(windows) {
                return Err("Archive size listing could not be parsed.".into());
            }

            continue;
        };

        files = files.saturating_add(1);
        bytes = bytes.saturating_add(size);

        if files > ARCHIVE_FILES || bytes > ARCHIVE_EXPANDED {
            return Err("Archive expands beyond its file or size limits.".into());
        }
    }

    Ok(())
}

pub(crate) fn tar_size(fields: &[&str]) -> Option<u64> {
    fields.iter().enumerate().skip(1).find_map(|(index, field)| {
        if !tar_date(field) {
            return None;
        }

        fields[..index].iter().rev().find_map(|value| value.parse::<u64>().ok())
    })
}

pub(crate) const TAR_MONTHS: [&str; 12] =
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

pub(crate) fn tar_date(value: &str) -> bool {
    let bytes = value.as_bytes();

    let iso_date = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..].iter().all(u8::is_ascii_digit);

    iso_date || TAR_MONTHS.contains(&value)
}

pub(crate) fn validate_archive(
    path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let name = path.to_string_lossy().to_ascii_lowercase();

    let (program, args): (&str, Vec<String>) = if name.ends_with(".zip") && cfg!(windows) {
        ("tar", vec!["-tf".into(), path.display().to_string()])
    } else if name.ends_with(".zip") {
        ("unzip", vec!["-Z1".into(), path.display().to_string()])
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        ("tar", vec!["-tzf".into(), path.display().to_string()])
    } else {
        ("tar", vec!["-tf".into(), path.display().to_string()])
    };

    let output = archive_listing(program, &args)?;

    for value in String::from_utf8_lossy(&output).lines() {
        let path = std::path::Path::new(value.trim());

        if path.is_absolute()
            || path.components().any(|part| part == std::path::Component::ParentDir)
        {
            return Err("Archive contains an unsafe path.".into());
        }
    }

    validate_archive_types(path, name.ends_with(".zip"))
}

pub(crate) fn validate_archive_types(
    path: &Path,
    zip: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (program, args): (&str, Vec<String>) = if zip && cfg!(windows) {
        ("tar", vec!["-tvf".into(), path.display().to_string()])
    } else if zip {
        ("unzip", vec!["-Z".into(), "-v".into(), path.display().to_string()])
    } else {
        ("tar", vec!["-tvf".into(), path.display().to_string()])
    };

    let output = archive_listing(program, &args)?;

    for line in String::from_utf8_lossy(&output).lines() {
        let kind = if zip && !cfg!(windows) {
            line.strip_prefix("  Unix file attributes (")
                .and_then(|value| value.split_once("): "))
                .and_then(|(_, value)| value.trim_start().as_bytes().first().copied())
        } else {
            line.as_bytes().first().copied()
        };

        if kind.is_some_and(|kind| !matches!(kind, b'-' | b'd')) {
            return Err("Archive contains a link or special file, which is not allowed.".into());
        }
    }

    Ok(())
}

pub(crate) fn archive_listing(
    program: &str,
    args: &[String],
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let output = command_output(std::process::Command::new(program).args(args))?;

    if !output.status.success() {
        return Err("Archive listing failed.".into());
    }

    Ok(output.stdout)
}

pub(crate) fn safe_archive(root: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = std::fs::canonicalize(root)?;
    let mut files = 0_usize;
    let mut bytes = 0_u64;

    safe_archive_at(&root, &mut files, &mut bytes)
}

pub(crate) fn safe_archive_at(
    root: &Path,
    files: &mut usize,
    bytes: &mut u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;

        if kind.is_symlink() {
            return Err("Archive contains a symbolic link, which is not allowed.".into());
        }

        #[cfg(unix)]
        if kind.is_file() && std::os::unix::fs::MetadataExt::nlink(&std::fs::metadata(&path)?) > 1 {
            return Err("Archive contains a hard link, which is not allowed.".into());
        }

        if !std::fs::canonicalize(&path)?.starts_with(root) {
            return Err("Archive contains a path outside its extraction root.".into());
        }

        if kind.is_dir() {
            safe_archive_at(&path, files, bytes)?;
        } else if kind.is_file() {
            *files = files.saturating_add(1);
            *bytes = bytes.saturating_add(std::fs::metadata(&path)?.len());

            if *files > ARCHIVE_FILES || *bytes > ARCHIVE_EXPANDED {
                return Err("Archive expands beyond its file or size limits.".into());
            }
        }
    }

    Ok(())
}

pub(crate) fn archive_root(
    temp: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    if temp.join("krabb-plugin.toml").is_file() {
        return Ok(temp.to_path_buf());
    }

    let roots = std::fs::read_dir(temp)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join("krabb-plugin.toml").is_file())
        .collect::<Vec<_>>();

    if roots.len() == 1 {
        Ok(roots[0].clone())
    } else {
        Err("Archive must contain one plugin root with krabb-plugin.toml.".into())
    }
}

pub(crate) fn download(
    url: &str,
    destination: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    download_with(url, destination, None, ARCHIVE_LIMIT)
}

/// Downloads a small document such as an index or checksum list, refusing one over `limit`.
pub(crate) fn download_bounded(
    url: &str,
    limit: u64,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let temp = temp("krabb-download")?;
    let path = temp.join("document");

    let result =
        download_with(url, &path, None, limit).and_then(|()| Ok(read_bounded(&path, limit)?));

    let _ = std::fs::remove_dir_all(&temp);

    result
}

/// Downloads at most `limit` bytes from `url`, sending the request headers stored in
/// `headers` to its origin only.
pub(crate) fn download_with(
    url: &str,
    destination: &Path,
    headers: Option<&Path>,
    limit: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let connect = DOWNLOAD_CONNECT.to_string();
    let time = DOWNLOAD_TIME.to_string();
    let root = destination.parent().ok_or("Archive destination has no staging directory.")?;
    let mut command = std::process::Command::new("curl");

    if let Some(headers) = headers {
        command.arg("--header").arg(format!("@{}", headers.display()));
    }

    let output = match command_output_limited(
        command
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--proto",
                "=https",
                "--tlsv1.2",
                "--connect-timeout",
                &connect,
                "--max-time",
                &time,
                "--max-filesize",
                &limit.to_string(),
                "--output",
            ])
            .arg(destination)
            .arg(url),
        Some(root),
        limit,
    ) {
        Ok(output) => output,

        Err(error) => {
            let _ = std::fs::remove_file(destination);
            return Err(error.into());
        }
    };

    if !output.status.success() {
        return Err(format!("Download failed: {}", sentence(redact(&output.stderr))).into());
    }

    if std::fs::metadata(destination)?.len() > limit {
        let _ = std::fs::remove_file(destination);

        return Err("Download exceeded the size limit.".into());
    }

    Ok(())
}
