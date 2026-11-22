//! Implements `krabb status`.

use crate::*;

pub(crate) async fn status_command(
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let value = status_at(&home()).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{}", status_text(&value, value["plugins"].as_u64().unwrap_or(0) as usize));
    }

    Ok(())
}

pub(crate) async fn status_at(root: &Path) -> Result<serde_json::Value, PluginError> {
    let config_text = load_config_text(root)?;
    let config_present = config_text.is_some();
    let config_ok = config_text.is_some_and(|text| {
        toml::from_str::<Config>(&text).is_ok_and(|config| config.validate().is_ok())
    });

    let lock = load_lock_at(root)?;
    let installed = lock.plugins.len();
    let config = load_config_at(root).unwrap_or_default();
    let (model, _) = selection::current(&config, (None, None), (None, None));
    let active = active_at(root).await?;
    let loaded = |id: &str| active.as_ref().is_some_and(|ids| ids.iter().any(|item| item == id));

    // Credential checks decrypt the store and may run a sign-in probe, so do each once.
    let configured = lock
        .plugins
        .keys()
        .filter(|id| !loaded(id))
        .map(|id| {
            (id.as_str(), ready_at(id, Some(&setup::path(root)), &credentials::key_path(root)))
        })
        .collect::<BTreeMap<_, _>>();

    let available = |id: &str| {
        if active.is_some() { loaded(id) } else { configured.get(id).copied().unwrap_or(false) }
    };

    let intelligence = capability_status(&lock, "model", available);
    let channels = capability_status(&lock, "channel", available);
    let model_ready = intelligence["plugins"].as_array().unwrap().iter().any(|id| id == &model);

    let channels_installed = lock
        .plugins
        .values()
        .any(|entry| entry.capabilities.iter().any(|capability| capability == "channel"));
    let channel = std::env::var("KRABB_CHANNEL").ok();
    let channels_ready = channels["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| channel.as_ref().is_none_or(|selected| id == selected));

    let conflicts = available_commands(root)?.values().any(|owners| owners.len() > 1);
    let (health, health_details) = status_health(
        config_ok,
        model_ready,
        channels_installed || channel.is_some(),
        channels_ready,
        conflicts,
    );

    let models_installed = lock
        .plugins
        .values()
        .any(|entry| entry.capabilities.iter().any(|capability| capability == "model"));
    let health_details = explain(health_details, config_present, models_installed);
    let daemon = if active.is_some() { "running" } else { "stopped" };

    let service = service_path().is_file();

    let states = lock
        .plugins
        .iter()
        .map(|(id, entry)| {
            let configured = configured.get(id.as_str()).copied().unwrap_or(false);

            plugin_state(id, entry, configured, active.as_deref(), service)
        })
        .collect::<Vec<_>>();

    if health == "healthy" {
        debug!(health, daemon, "Status check completed.");
    } else {
        warn!(health, daemon, details = %health_details.join("; "), "Status check found unhealthy state.");
    }

    Ok(serde_json::json!({
        "version": VERSION,
        "health": health,
        "health_details": health_details,
        "daemon": daemon,
        "intelligence": intelligence,
        "channels": channels,
        "plugins": installed,
        "plugin_states": states,
        "recovery": upgrade::recovery(root),
    }))
}

/// Places one installed plugin on the installed, configured, running, and ready path.
///
/// Each state other than ready names the command that moves the plugin forward. Clients
/// run when the user opens them, so the daemon never reports them as failed.
pub(crate) fn plugin_state(
    id: &str,
    entry: &Entry,
    configured: bool,
    active: Option<&[String]>,
    service: bool,
) -> serde_json::Value {
    let client = entry.capabilities.iter().all(|capability| capability == "client");
    let loaded = active.is_some_and(|ids| ids.iter().any(|item| item == id));

    let (state, next) = if loaded || (client && configured) {
        ("ready", None)
    } else if !configured {
        ("needs setup", Some(format!("krabb plugin setup {id}")))
    } else if active.is_none() {
        let start = if service { "krabb service start" } else { "krabb service install" };

        ("configured", Some(start.to_owned()))
    } else {
        let log = logging::application_log_path();

        ("failed to start", Some(format!("check {} then run krabb service restart", log.display())))
    };

    serde_json::json!({"id": id, "state": state, "next": next})
}

pub(crate) fn status_health(
    config_ok: bool,
    model_ready: bool,
    channels_expected: bool,
    channels_ready: bool,
    conflicts: bool,
) -> (&'static str, Vec<&'static str>) {
    let mut details = Vec::new();

    if !config_ok {
        details.push("configuration is invalid");
    }

    if !model_ready {
        details.push("selected intelligence plugin is not ready");
    }

    if channels_expected && !channels_ready {
        details.push("no channel is ready or the selected channel is not ready");
    }

    if conflicts {
        details.push("plugin commands have name conflicts");
    }

    let health = if details.is_empty() { "healthy" } else { "unhealthy" };

    (health, details)
}

/// Replaces a failed check with its cause and next command when the home is incomplete.
pub(crate) fn explain(
    details: Vec<&'static str>,
    config_present: bool,
    models_installed: bool,
) -> Vec<&'static str> {
    details
        .into_iter()
        .map(|detail| match detail {
            "configuration is invalid" if !config_present => {
                "configuration is missing (run `krabb init`)"
            }

            "selected intelligence plugin is not ready" if !models_installed => {
                "no intelligence plugin is installed (run `krabb plugin install <provider>`)"
            }

            detail => detail,
        })
        .collect()
}

pub(crate) fn capability_status(
    lock: &Lock,
    capability: &str,
    available: impl Fn(&str) -> bool,
) -> serde_json::Value {
    let plugins = lock
        .plugins
        .iter()
        .filter(|(id, entry)| {
            entry.capabilities.iter().any(|item| item == capability) && available(id)
        })
        .map(|(id, _)| id.as_str())
        .collect::<Vec<_>>();

    serde_json::json!({
        "status": if plugins.is_empty() { "not configured" } else { "configured" },
        "plugins": plugins,
    })
}

pub(crate) fn capability_text(value: &serde_json::Value) -> String {
    let status = value["status"].as_str().unwrap_or("not configured");
    let plugins = value["plugins"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>();

    if status == "configured" && !plugins.is_empty() {
        format!("configured ({})", plugins.join(", "))
    } else {
        status.into()
    }
}

pub(crate) fn status_text(value: &serde_json::Value, installed: usize) -> String {
    let mut output = format!(
        "{:<20}{}\n{:<20}{}\n{:<20}{}\n{:<20}{}\n{:<20}{}\n{:<20}{} installed",
        "Version:",
        value["version"].as_str().unwrap_or(VERSION),
        "Health:",
        value["health"].as_str().unwrap_or("unhealthy"),
        "Background runtime:",
        value["daemon"].as_str().unwrap_or("stopped"),
        "Intelligence:",
        capability_text(&value["intelligence"]),
        "Channels:",
        capability_text(&value["channels"]),
        "Plugins:",
        installed
    );

    for plugin in value["plugin_states"].as_array().into_iter().flatten() {
        let id = plugin["id"].as_str().unwrap_or_default();
        let state = plugin["state"].as_str().unwrap_or_default();

        output.push_str(&format!("\n  {id:<18}{state}"));

        if let Some(next) = plugin["next"].as_str() {
            output.push_str(&format!(": {next}"));
        }
    }

    if let Some(details) = value["health_details"].as_array() {
        let details = details.iter().filter_map(serde_json::Value::as_str).collect::<Vec<_>>();

        if !details.is_empty() {
            output.push_str("\nHealth details:     ");
            output.push_str(&details.join(", "));
        }
    }

    if let Some(from) = value["recovery"]["from"].as_str() {
        let state = if value["recovery"]["accepted"] == true {
            "kept"
        } else {
            "update unfinished; run `krabb update --rollback`"
        };

        output.push_str(&format!("\n{:<20}Krabb {from} ({state})", "Recovery point:"));
    }

    output
}
