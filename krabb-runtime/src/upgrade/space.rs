//! Checks free disk space before an update stops the service or writes anything.

use std::io::ErrorKind;

use super::*;

/// Headroom kept free on every disk the update writes to.
const MARGIN: u64 = 64 * 1024 * 1024;

/// Estimated unpacked size of a plugin archive relative to its compressed size.
const EXPANSION: u64 = 3;

/// Refuses the update when a disk it writes to lacks room for the recovery point and new files.
pub(super) fn check(
    root: &Path,
    binaries: &[PathBuf],
    staged: &stage::Staged,
) -> Result<(), PluginError> {
    let replacements = [staged.krabb.clone(), staged.daemon.clone()];
    let archives = staged.plan.plugins.iter().map(|artifact| staged.dir.join(&artifact.file));
    let disks =
        Disks { root, binaries, replacements: &replacements, temporary: &std::env::temp_dir() };

    fits(&disks, &archives.collect::<Vec<_>>(), krabb_file::available)
}

/// Where an update writes: the home, each binary's directory, and the unpacking directory.
struct Disks<'a> {
    root: &'a Path,
    binaries: &'a [PathBuf],
    replacements: &'a [PathBuf],
    temporary: &'a Path,
}

fn fits(
    disks: &Disks<'_>,
    archives: &[PathBuf],
    available: impl Fn(&Path) -> std::io::Result<u64>,
) -> Result<(), PluginError> {
    let mut needs = vec![(disks.root.to_path_buf(), home(disks.root, disks.binaries, archives)?)];

    for (binary, replacement) in disks.binaries.iter().zip(disks.replacements) {
        let directory = binary.parent().ok_or("Binary has no parent directory.")?;

        needs.push((directory.to_path_buf(), size(replacement)?));
    }

    // Activation unpacks one plugin archive at a time in the temporary directory.
    let sizes = archives.iter().map(|archive| size(archive)).collect::<Result<Vec<_>, _>>()?;
    let largest = sizes.into_iter().max().unwrap_or(0).saturating_mul(EXPANSION);

    needs.push((disks.temporary.to_path_buf(), largest));

    for (path, need) in group(needs)? {
        let need = need.saturating_add(MARGIN);
        let free = available(&path).map_err(|error| located(&path, error))?;

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

/// Home bytes still to be written: the new recovery point, which first replaces the old one,
/// and the unpacked plugins.
fn home(root: &Path, binaries: &[PathBuf], archives: &[PathBuf]) -> Result<u64, PluginError> {
    let mut total = 0_u64;

    for path in snapshot::paths(root)? {
        total = total.saturating_add(size(&root.join(path))?);
    }

    for binary in binaries {
        total = total.saturating_add(size(binary)?);
    }

    for archive in archives {
        total = total.saturating_add(size(archive)?.saturating_mul(EXPANSION));
    }

    Ok(total.saturating_sub(size(&snapshot::store(root))?))
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

    let metadata = std::fs::metadata(path).map_err(|error| located(path, error))?;

    Ok(metadata.dev().to_string())
}

#[cfg(not(unix))]
fn disk(path: &Path) -> Result<String, PluginError> {
    use std::path::{Component, Prefix};

    let absolute = std::path::absolute(path).map_err(|error| located(path, error))?;

    let disk = match absolute.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                (letter as char).to_ascii_lowercase().to_string()
            }

            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                format!("{}\\{}", server.to_string_lossy(), share.to_string_lossy())
                    .to_ascii_lowercase()
            }

            _ => prefix.as_os_str().to_string_lossy().to_ascii_lowercase(),
        },

        _ => String::new(),
    };

    Ok(disk)
}

/// Bytes `path` occupies, without following symbolic links; paths that vanish count as empty.
fn size(path: &Path) -> Result<u64, PluginError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(located(path, error)),
    };

    if !metadata.is_dir() {
        return Ok(occupied(&metadata));
    }

    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(located(path, error)),
    };

    let mut total = occupied(&metadata);

    for entry in entries {
        match entry {
            Ok(entry) => total = total.saturating_add(size(&entry.path())?),

            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(located(path, error)),
        }
    }

    Ok(total)
}

/// Allocated blocks where the platform reports them, so many small files are not undercounted.
#[cfg(unix)]
fn occupied(metadata: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;

    metadata.len().max(metadata.blocks().saturating_mul(512))
}

#[cfg(not(unix))]
fn occupied(metadata: &std::fs::Metadata) -> u64 {
    metadata.len()
}

fn located(path: &Path, error: std::io::Error) -> PluginError {
    format!("Could not measure {}: {}.", path.display(), sentence(error.to_string())).into()
}

/// Mebibytes rounded up to one decimal, so a shortfall never reads as nothing.
fn mebibytes(bytes: u64) -> String {
    let tenths = bytes.saturating_mul(10).div_ceil(1024 * 1024);

    format!("{}.{} MiB", tenths / 10, tenths % 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_trees_without_following_links_and_groups_disks() {
        let root = crate::tests::test_root("update-space");
        let nested = root.join("data/sessions");

        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("index.json"), vec![1; 50]).unwrap();

        let plain = size(&nested).unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, nested.join("loop")).unwrap();

        assert!(plain >= 50);
        #[cfg(unix)]
        let link = occupied(&std::fs::symlink_metadata(nested.join("loop")).unwrap());

        #[cfg(not(unix))]
        let link = 0;

        let entries = [nested.clone(), nested.join("index.json")]
            .iter()
            .map(|path| occupied(&std::fs::symlink_metadata(path).unwrap()))
            .sum::<u64>();

        assert_eq!(size(&nested).unwrap(), entries + link);
        assert_eq!(size(&root.join("missing")).unwrap(), 0);

        let grouped = group(vec![(root.clone(), 10), (nested.clone(), 5)]).unwrap();

        assert_eq!(grouped, vec![(root.clone(), 15)]);
        assert_eq!(mebibytes(3 * 1024 * 1024 / 2), "1.5 MiB");
        assert_eq!(mebibytes(1), "0.1 MiB");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn refuses_updates_that_cannot_fit_and_credits_the_old_point() {
        let root = crate::tests::test_root("update-space-refusal");
        let home = root.join("home");
        let bin = root.join("bin");
        let binaries = [bin.join("krabb")];
        let replacements = [root.join("staged-krabb")];
        let archives = [root.join("echo.tar.gz")];

        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(home.join("config.toml"), vec![1; 1024]).unwrap();
        std::fs::write(&binaries[0], vec![1; 2048]).unwrap();
        std::fs::write(&replacements[0], vec![1; 4096]).unwrap();
        std::fs::write(&archives[0], vec![1; 100]).unwrap();

        let disks = Disks {
            root: &home,
            binaries: &binaries,
            replacements: &replacements,
            temporary: &root,
        };

        let measured = |path: &Path| size(path).unwrap();
        let need = measured(&home.join("config.toml"))
            + measured(&binaries[0])
            + measured(&replacements[0])
            + 2 * measured(&archives[0]) * EXPANSION
            + MARGIN;

        fits(&disks, &archives, |_| Ok(need)).unwrap();

        let error = fits(&disks, &archives, |_| Ok(need - 1)).unwrap_err().to_string();

        assert!(error.contains("nothing was changed"), "{error}");
        assert!(error.contains("Free 0.1 MiB"), "{error}");

        let old = snapshot::store(&home).join("home/config.toml");

        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, vec![1; 8192]).unwrap();

        fits(&disks, &archives, |_| Ok(need - 1)).unwrap();

        let _ = std::fs::remove_dir_all(root);
    }
}
