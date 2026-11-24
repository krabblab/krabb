//! Locates, initializes, and resets the Krabb home.

use crate::*;

pub(crate) fn binary_at(id: &str, root: &Path) -> Option<PathBuf> {
    if !valid(id) {
        return None;
    }

    let config =
        root.join("plugins").join(id).join("bin").join(krabb_core::plugin::binary_name(id));

    if config.is_file() {
        return Some(config);
    }

    None
}

pub(crate) fn valid(id: &str) -> bool {
    !id.is_empty()
        && id.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(crate) fn home() -> PathBuf {
    std::env::var_os("KRABB_HOME").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map_or_else(|| PathBuf::from("."), PathBuf::from)
                    .join(".config")
            })
            .join(NAME)
    })
}

pub(crate) fn workspace_root_at(home_root: &Path) -> PathBuf {
    std::env::var_os("KRABB_ROOT").map_or_else(|| home_root.join("workspace"), PathBuf::from)
}

pub(crate) fn workspace_root() -> PathBuf {
    workspace_root_at(&home())
}

pub(crate) fn init(
    force: bool,
    yes: bool,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = home();

    validate_init_flags(force, yes)?;

    let reset_existing = force && root.exists();

    if regular_file_present(&root.join("config.toml")) && !force {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "home": root,
                    "initialized": false,
                    "status": "already_initialized"
                })
            );
        } else {
            println!("{}", already(&root)?);
        }

        return Ok(());
    }

    if reset_existing
        && !confirm_action(
            yes,
            json,
            &format!("Reset Krabb and permanently delete all data at {}?", root.display()),
        )?
    {
        if json {
            println!("{}", serde_json::json!({"home": root, "status": "cancelled"}));
        } else {
            println!("Krabb reset cancelled.");
        }

        return Ok(());
    }

    init_at_with_force(&root, reset_existing)?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "home": root,
                "initialized": true,
                "status": if force { "reinitialized" } else { "initialized" }
            })
        );
    } else {
        let home = crate::format_tree_list(&[format!("Home: {}", root.display())]);

        println!(
            "Krabb is initialized\n{home}\n\nAdd an intelligence provider with `krabb init \
             provider`."
        );
    }

    info!(reset_existing, "Krabb home initialized.");

    Ok(())
}

pub(crate) fn validate_init_flags(force: bool, yes: bool) -> Result<(), &'static str> {
    if yes && !force {
        return Err("init --yes requires --force.");
    }

    Ok(())
}

pub(crate) fn init_at(root: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    init_at_with_force(root, false)
}

pub(crate) fn init_at_with_force(
    root: &Path,
    force: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if force && root.exists() {
        reset_home(root)?;
        return Ok(());
    }

    schema::check(root)?;
    initialize_home(root, false)?;

    Ok(())
}

pub(crate) fn initialize_home(root: &Path, force_config: bool) -> std::io::Result<()> {
    ensure_real_directory(root)?;
    ensure_real_directory(&root.join("plugins"))?;
    ensure_real_directory(&root.join("data"))?;
    ensure_real_directory(&root.join("data/schedules"))?;
    ensure_real_directory(&root.join("logs"))?;
    ensure_real_directory(&root.join("logs/debug"))?;
    ensure_real_directory(&root.join("logs/schedules"))?;

    for relative in [
        "run",
        "cache",
        "data/plugins",
        "data/schedules/runs",
        "data/system",
        "data/workspaces",
        "worktrees",
    ] {
        krabb_file::directory(root, Path::new(relative))?;
    }

    let workspace = root.join("workspace");
    ensure_real_directory(&workspace)?;

    #[cfg(unix)]
    {
        for directory in [
            root,
            root.join("plugins").as_path(),
            root.join("data").as_path(),
            root.join("data/schedules").as_path(),
            root.join("logs").as_path(),
            root.join("logs/debug").as_path(),
            root.join("logs/schedules").as_path(),
            workspace.as_path(),
        ] {
            let mut permissions = std::fs::metadata(directory)?.permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(directory, permissions)?;
        }
    }

    {
        let _lock = metadata_lock(root)?;
        let config_path = root.join("config.toml");
        let existing = load_metadata(&config_path, LOCK_LIMIT)?;

        if force_config || existing.is_none() {
            let config =
                toml::to_string_pretty(&Config::default()).map_err(std::io::Error::other)?;
            save_file(&config_path, config.as_bytes())?;
        }
    }

    let _lock = metadata_lock(root)?;

    for (name, template) in [("KRAB.md", templates::KRAB), ("KLAW.md", templates::KLAW)] {
        let destination = root.join("data/system").join(name);
        let previous = workspace.join(name);

        if let Some(bytes) = load_metadata(&previous, LOCK_LIMIT)? {
            if let Some(current) = load_metadata(&destination, LOCK_LIMIT)? {
                if current != bytes {
                    return Err(std::io::Error::other(
                        "Conflicting system instructions, so reconcile workspace and data/system before starting.",
                    ));
                }
            } else {
                krabb_file::save(&destination, bytes)?;
            }

            std::fs::remove_file(previous)?;
        }

        create_instruction_file(&destination, template)?;
    }

    Ok(())
}

pub(crate) fn ensure_real_directory(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Expected a real directory at {}.", path.display()),
            ))
        }

        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir_all(path),
        Err(error) => Err(error),
    }
}

pub(crate) fn create_instruction_file(path: &Path, contents: &str) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("Instruction file cannot be a symbolic link: {}.", path.display()),
        )),
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Instruction path must be a regular file: {}.", path.display()),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => secure(path, contents),
        Err(error) => Err(error),
    }
}

pub(crate) fn reset_home(root: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(root)?;

    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Krabb home must be a real directory before it can be reset.",
        ));
    }

    if root.parent().is_none() || root.file_name().is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Refusing to reset a filesystem root or unnamed directory.",
        ));
    }

    let _daemon = owner_lock(root).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "Stop the Krabb daemon before resetting its home.",
            )
        } else {
            error
        }
    })?;

    let _plugins = lock_at(root, ".plugins.lock").map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "Wait for plugin management to finish before resetting Krabb.",
            )
        } else {
            error
        }
    })?;

    let metadata = metadata_lock(root)?;

    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();

        if name == "daemon.lock" || name == ".plugins.lock" || name == ".metadata.lock" {
            continue;
        }

        if name == "run" {
            for runtime in std::fs::read_dir(entry.path())? {
                let runtime = runtime?;

                if runtime.file_name() != "daemon.lock" {
                    if runtime.file_type()?.is_dir() {
                        std::fs::remove_dir_all(runtime.path())?;
                    } else {
                        std::fs::remove_file(runtime.path())?;
                    }
                }
            }

            continue;
        }

        let path = entry.path();

        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
    }

    drop(metadata);
    initialize_home(root, true)
}

pub(crate) fn directory_present(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
}

pub(crate) fn regular_file_present(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
}

/// The answer to `krabb init` on a home that exists, with the step that is still missing.
fn already(root: &Path) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let home = crate::format_tree_list(&[format!("Home: {}", root.display())]);

    let next = if crate::onboard::ready_provider(root)?.is_some() {
        "Check every part with `krabb status`."
    } else {
        "Add an intelligence provider with `krabb init provider`."
    };

    Ok(format!("Krabb is already initialized\n{home}\n\n{next}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_existing_home_names_its_missing_step() {
        let root = crate::tests::test_root("home-already");

        init_at(&root).unwrap();

        let text = already(&root).unwrap();

        assert!(text.starts_with("Krabb is already initialized\n└─ Home: "), "{text}");
        assert!(text.ends_with("\n\nAdd an intelligence provider with `krabb init provider`."));

        let _ = std::fs::remove_dir_all(root);
    }
}
