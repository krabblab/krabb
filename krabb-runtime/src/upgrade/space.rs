//! Checks free disk space before an update stops the service or writes anything.

use super::*;

/// Headroom kept free on every disk the update writes to.
const MARGIN: u64 = 64 * 1024 * 1024;

/// Staged plugin archives unpack into the home; compressed size understates that.
const UNPACKED: u64 = 3;

/// Refuses the update when a disk it writes to lacks room for the recovery point and new files.
pub(super) fn check(
    root: &Path,
    binaries: &[PathBuf],
    staged: &stage::Staged,
) -> Result<(), PluginError> {
    let replacements = [staged.krabb.clone(), staged.daemon.clone()];
    let archives = staged.plan.plugins.iter().map(|artifact| staged.dir.join(&artifact.file));

    check_with(root, binaries, &replacements, &archives.collect::<Vec<_>>(), krabb_file::available)
}

fn check_with(
    root: &Path,
    binaries: &[PathBuf],
    replacements: &[PathBuf],
    archives: &[PathBuf],
    available: impl Fn(&Path) -> std::io::Result<u64>,
) -> Result<(), PluginError> {
    let mut needs = vec![(root.to_path_buf(), recovery(root, binaries)? + unpacked(archives)?)];

    for (binary, replacement) in binaries.iter().zip(replacements) {
        let directory = binary.parent().ok_or("Binary has no parent directory.")?;

        needs.push((directory.to_path_buf(), size(replacement)?));
    }

    for (path, need) in group(needs)? {
        let need = need.saturating_add(MARGIN);
        let free = available(&path)?;

        if free < need {
            return Err(format!(
                "Updating needs {} free on the disk holding {}, but only {} is available.\nFree \
                 {} and try again; nothing was changed.",
                mebibytes(need),
                path.display(),
                mebibytes(free),
                mebibytes(need - free)
            )
            .into());
        }
    }

    Ok(())
}

/// Bytes the recovery point copies: the saved home state plus the current binaries.
fn recovery(root: &Path, binaries: &[PathBuf]) -> Result<u64, PluginError> {
    let mut total = 0_u64;

    for path in snapshot::paths(root)? {
        total = total.saturating_add(size(&root.join(path))?);
    }

    for binary in binaries {
        total = total.saturating_add(size(binary)?);
    }

    Ok(total)
}

fn unpacked(archives: &[PathBuf]) -> Result<u64, PluginError> {
    let mut total = 0_u64;

    for archive in archives {
        total = total.saturating_add(size(archive)?.saturating_mul(UNPACKED));
    }

    Ok(total)
}

/// Sums needs that land on the same disk, keyed by the first path seen for each disk.
fn group(needs: Vec<(PathBuf, u64)>) -> Result<Vec<(PathBuf, u64)>, PluginError> {
    let mut grouped: Vec<(String, PathBuf, u64)> = Vec::new();

    for (path, need) in needs {
        let disk = disk(&path)?;

        match grouped.iter_mut().find(|(known, ..)| *known == disk) {
            Some((_, _, total)) => *total = total.saturating_add(need),
            None => grouped.push((disk, path, need)),
        }
    }

    Ok(grouped.into_iter().map(|(_, path, need)| (path, need)).collect())
}

#[cfg(unix)]
fn disk(path: &Path) -> Result<String, PluginError> {
    use std::os::unix::fs::MetadataExt;

    Ok(std::fs::metadata(path)?.dev().to_string())
}

#[cfg(not(unix))]
fn disk(path: &Path) -> Result<String, PluginError> {
    let absolute = std::path::absolute(path)?;
    let prefix = absolute.components().next().map(|part| part.as_os_str().to_owned());

    Ok(prefix.unwrap_or_default().to_string_lossy().to_ascii_lowercase())
}

/// Total bytes under `path`, without following symbolic links; a missing path is empty.
fn size(path: &Path) -> Result<u64, PluginError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };

    if !metadata.is_dir() {
        return Ok(metadata.len());
    }

    let mut total = 0_u64;

    for entry in std::fs::read_dir(path)? {
        total = total.saturating_add(size(&entry?.path())?);
    }

    Ok(total)
}

fn mebibytes(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_trees_without_following_links_and_groups_disks() {
        let root = crate::tests::test_root("update-space");
        let nested = root.join("data/sessions");

        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("config.toml"), vec![0; 100]).unwrap();
        std::fs::write(nested.join("index.json"), vec![0; 50]).unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink("/", root.join("data/outside")).unwrap();

        assert_eq!(size(&nested).unwrap(), 50);
        assert_eq!(size(&root.join("missing")).unwrap(), 0);
        assert!(size(&root.join("data")).unwrap() < 4096);

        let grouped = group(vec![(root.clone(), 10), (nested.clone(), 5)]).unwrap();

        assert_eq!(grouped, vec![(root.clone(), 15)]);
        assert_eq!(mebibytes(3 * 1024 * 1024 / 2), "1.5 MiB");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn refuses_updates_that_cannot_fit_without_changing_anything() {
        let root = crate::tests::test_root("update-space-refusal");
        let home = root.join("home");
        let bin = root.join("bin");
        let binary = bin.join("krabb");
        let replacement = root.join("staged-krabb");
        let archive = root.join("echo.tar.gz");

        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(home.join("config.toml"), vec![0; 1024]).unwrap();
        std::fs::write(&binary, vec![0; 2048]).unwrap();
        std::fs::write(&replacement, vec![0; 4096]).unwrap();
        std::fs::write(&archive, vec![0; 100]).unwrap();

        let binaries = [binary];
        let replacements = [replacement];
        let archives = [archive];
        let need = 1024 + 2048 + 4096 + 300 + MARGIN;

        check_with(&home, &binaries, &replacements, &archives, |_| Ok(need)).unwrap();

        let error = check_with(&home, &binaries, &replacements, &archives, |_| Ok(need - 1))
            .unwrap_err()
            .to_string();

        assert!(error.contains("nothing was changed"), "{error}");
        assert!(error.contains("Free 0.0 MiB"), "{error}");
        assert!(!snapshot::store(&home).exists());

        let _ = std::fs::remove_dir_all(root);
    }
}
