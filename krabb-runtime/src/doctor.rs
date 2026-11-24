//! Implements `krabb doctor`, which checks the home, plugins, and daemon.

use crate::*;

pub(crate) fn installed_at(root: &Path) -> Vec<String> {
    let root = root.join("plugins");

    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };

    let mut ids = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .collect::<Vec<_>>();

    ids.sort();
    ids
}

pub(crate) fn doctor(
    fix: bool,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    doctor_at(&home(), fix, json)
}

pub(crate) fn doctor_healthy(
    config_present: bool,
    config_valid: Option<bool>,
    plugins_directory_present: bool,
    workspace_directory_present: bool,
    krab_instructions_present: bool,
    klaw_instructions_present: bool,
) -> bool {
    config_present
        && config_valid == Some(true)
        && plugins_directory_present
        && workspace_directory_present
        && krab_instructions_present
        && klaw_instructions_present
}

pub(crate) fn doctor_at(
    root: &Path,
    fix: bool,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    schema::check(root)?;

    let config_path = root.join("config.toml");
    let plugins_path = root.join("plugins");
    let workspace_path = root.join("workspace");
    let krab_path = root.join("data/system/KRAB.md");
    let klaw_path = root.join("data/system/KLAW.md");
    let config_was_present = load_config_text(root)?.is_some();
    let plugins_directory_was_present = directory_present(&plugins_path);
    let workspace_directory_was_present = directory_present(&workspace_path);
    let krab_instructions_were_present = regular_file_present(&krab_path);
    let klaw_instructions_were_present = regular_file_present(&klaw_path);
    let mut repairs = Vec::new();

    if fix {
        init_at(root)?;

        if !config_was_present && regular_file_present(&config_path) {
            repairs.push("created_config");
        }

        if !plugins_directory_was_present && directory_present(&plugins_path) {
            repairs.push("created_plugins_directory");
        }

        if !workspace_directory_was_present && directory_present(&workspace_path) {
            repairs.push("created_workspace_directory");
        }

        if !krab_instructions_were_present && regular_file_present(&krab_path) {
            repairs.push("created_krab_instructions");
        }

        if !klaw_instructions_were_present && regular_file_present(&klaw_path) {
            repairs.push("created_klaw_instructions");
        }
    }

    let config_present = regular_file_present(&config_path);
    let plugins_directory_present = directory_present(&plugins_path);
    let workspace_directory_present = directory_present(&workspace_path);
    let krab_instructions_present = regular_file_present(&krab_path);
    let klaw_instructions_present = regular_file_present(&klaw_path);
    let mut config_valid = None;

    if config_present {
        let config: Config = load_config_at(root)?;

        config.validate().map_err(|error| format!("Config is invalid: {error}"))?;
        config_valid = Some(true);
    }

    let lock = load_lock_at(root)?;

    for (id, entry) in &lock.plugins {
        let binary =
            binary_at(id, root).ok_or_else(|| format!("Plugin binary is missing: {id}."))?;

        let manifest = root.join("plugins").join(id).join("krabb-plugin.toml");

        verify_plugin_binary(id, entry, &manifest, &binary)?;
    }

    let healthy = doctor_healthy(
        config_present,
        config_valid,
        plugins_directory_present,
        workspace_directory_present,
        krab_instructions_present,
        klaw_instructions_present,
    );

    if fix {
        if repairs.is_empty() {
            info!(healthy, "Doctor repair completed with no changes needed.");
        } else {
            info!(repair_count = repairs.len(), healthy, "Doctor applied repairs.");
        }
    } else if healthy {
        info!("Doctor check passed.");
    } else {
        warn!("Doctor check found unhealthy local state.");
    }

    let health = if healthy {
        serde_json::json!({"status": "healthy"})
    } else {
        serde_json::json!({
            "status": "unhealthy",
            "suggestion": "krabb doctor --fix"
        })
    };

    if json {
        if fix {
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({"repairs": repairs}))?);
        } else {
            let value = serde_json::json!({
                "config": {"present": config_present, "valid": config_valid},
                "health": health,
                "home": root,
                "plugin_integrity": "valid",
                "plugins": {
                    "directory_present": plugins_directory_present,
                    "installed": lock.plugins.keys().collect::<Vec<_>>(),
                },

                "workspace": {
                    "directory_present": workspace_directory_present,
                    "krab_instructions_present": krab_instructions_present,
                    "klaw_instructions_present": klaw_instructions_present,
                },

                "protocol": {
                    "major": Protocol::CURRENT.major,
                    "minor": Protocol::CURRENT.minor,
                },
            });

            println!("{}", serde_json::to_string_pretty(&value)?);
        }
    } else if fix {
        if repairs.is_empty() {
            println!("No repairs were needed.");
        } else {
            let descriptions = repairs
                .iter()
                .map(|repair| match *repair {
                    "created_config" => "default configuration",
                    "created_plugins_directory" => "plugins directory",
                    "created_workspace_directory" => "workspace directory",
                    "created_krab_instructions" => "KRAB.md instructions",
                    "created_klaw_instructions" => "KLAW.md instructions",
                    _ => repair,
                })
                .map(str::to_owned)
                .collect::<Vec<_>>();

            println!("Repairs applied:\n{}", format_tree_list(&descriptions));
        }
    } else {
        let checks = Checks {
            configuration: config_valid == Some(true),
            plugins: plugins_directory_present,
            workspace: workspace_directory_present,
            krab: krab_instructions_present,
            klaw: klaw_instructions_present,
        };

        println!("{}", report(root, &checks, lock.plugins.len(), healthy));
    }

    Ok(())
}

/// What the read-only check found in the home.
struct Checks {
    configuration: bool,
    plugins: bool,
    workspace: bool,
    krab: bool,
    klaw: bool,
}

/// The check as the plugin tree layout, ending with the one command to run next.
fn report(root: &Path, checks: &Checks, installed: usize, healthy: bool) -> String {
    let present = |found: bool| if found { "present" } else { "missing" };

    let configuration = if checks.configuration { "valid" } else { "missing" };

    let headline = if healthy { "is healthy" } else { "needs repair" };

    let parts = [
        format!("Home: {}", root.display()),
        format!("Configuration: {configuration}"),
        format!("Plugins directory: {}", present(checks.plugins)),
        format!("Workspace: {}", present(checks.workspace)),
        format!("KRAB.md instructions: {}", present(checks.krab)),
        format!("KLAW.md instructions: {}", present(checks.klaw)),
        format!(
            "Plugins: {installed} installed, integrity checks passed, protocol {}.{}",
            Protocol::CURRENT.major,
            Protocol::CURRENT.minor
        ),
    ];

    let next = if healthy {
        "Check that the runtime and plugins are running with `krabb status`."
    } else {
        "Restore the missing local state with `krabb doctor --fix`."
    };

    format!("The local installation {headline}\n{}\n\n{next}", format_tree_list(&parts))
}

#[cfg(test)]
mod layout {
    use super::*;

    #[test]
    fn the_check_reads_as_a_tree_with_one_next_action() {
        let all =
            Checks { configuration: true, plugins: true, workspace: true, krab: true, klaw: true };

        assert_eq!(
            report(Path::new("/home/owner/.krabb"), &all, 2, true),
            "The local installation is healthy\n\
             ├─ Home: /home/owner/.krabb\n\
             ├─ Configuration: valid\n\
             ├─ Plugins directory: present\n\
             ├─ Workspace: present\n\
             ├─ KRAB.md instructions: present\n\
             ├─ KLAW.md instructions: present\n\
             └─ Plugins: 2 installed, integrity checks passed, protocol 0.1\n\
             \n\
             Check that the runtime and plugins are running with `krabb status`."
        );

        let missing = Checks { workspace: false, ..all };
        let text = report(Path::new("/home/owner/.krabb"), &missing, 0, false);

        assert!(text.starts_with("The local installation needs repair\n"), "{text}");
        assert!(text.contains("├─ Workspace: missing\n"), "{text}");
        assert!(text.ends_with("\n\nRestore the missing local state with `krabb doctor --fix`."));
    }
}
