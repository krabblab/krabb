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
    status_with(root, runtime_at(root).await?)
}

/// The status of the home at ROOT, given the plugins a running daemon loaded.
pub(crate) fn status_with(
    root: &Path,
    runtime: Option<Runtime>,
) -> Result<serde_json::Value, PluginError> {
    let config_text = load_config_text(root)?;
    let config_present = config_text.is_some();
    let config_ok = config_text.is_some_and(|text| {
        toml::from_str::<Config>(&text).is_ok_and(|config| config.validate().is_ok())
    });

    let lock = load_lock_at(root)?;
    let installed = lock.plugins.len();
    let active = runtime.as_ref().map(|runtime| runtime.active.clone());
    let config = load_config_at(root).unwrap_or_default();
    let (model, _) = selection::current(&config, (None, None), (None, None));
    let loaded = |id: &str| active.as_ref().is_some_and(|ids| ids.iter().any(|item| item == id));
    let configured = configured_at(root, &lock, active.as_deref());

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

    let states = states(root, &lock, &configured, runtime);

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
        "recovered": upgrade::outcome(root),
    }))
}

/// Each installed plugin's place on the installed, configured, running, and ready path.
pub(crate) fn plugin_states(root: &Path, lock: &Lock, runtime: Option<Runtime>) -> Vec<Value> {
    let active = runtime.as_ref().map(|runtime| runtime.active.clone());
    let configured = configured_at(root, lock, active.as_deref());

    states(root, lock, &configured, runtime)
}

/// Whether each plugin the daemon did not load has its credentials.
///
/// Credential checks decrypt the store and may run a sign-in probe, so each runs once.
fn configured_at(root: &Path, lock: &Lock, active: Option<&[String]>) -> BTreeMap<String, bool> {
    let loaded = |id: &str| active.is_some_and(|ids| ids.iter().any(|item| item == id));

    lock.plugins
        .keys()
        .filter(|id| !loaded(id))
        .map(|id| {
            let ready = ready_at(id, Some(&setup::path(root)), &credentials::key_path(root));

            (id.clone(), ready)
        })
        .collect()
}

fn states(
    root: &Path,
    lock: &Lock,
    configured: &BTreeMap<String, bool>,
    runtime: Option<Runtime>,
) -> Vec<Value> {
    let service = service_path().is_file();
    let active = runtime.as_ref().map(|runtime| runtime.active.clone());
    let failures = runtime.as_ref().map(|runtime| runtime.failures.clone()).unwrap_or_default();

    // A running daemon checks programs with the service's PATH, which may differ from this one.
    let missing =
        runtime.map_or_else(|| requirement::missing(root, lock), |runtime| runtime.missing);

    lock.plugins
        .iter()
        .map(|(id, entry)| {
            let configured = configured.get(id).copied().unwrap_or(false);

            let observed = Observed {
                active: active.as_deref(),
                failure: failures.get(id),
                missing: missing.get(id).map(String::as_str),
            };

            plugin_state(id, entry, configured, &observed, service)
        })
        .collect()
}

/// What the daemon, or this process when none runs, observed about one plugin.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Observed<'a> {
    /// The plugins a running daemon loaded; none when the daemon is stopped.
    pub active: Option<&'a [String]>,
    /// Why the daemon could not start the plugin, when it tried and failed.
    pub failure: Option<&'a StartFailure>,
    /// How to install a program the plugin needs and cannot find.
    pub missing: Option<&'a str>,
}

/// The command that sets up plugin ID; Codex has no settings to enter and signs in instead.
fn setup_command(id: &str) -> String {
    if id == "codex" { "krabb codex login".into() } else { format!("krabb plugin setup {id}") }
}

/// Places one installed plugin on the installed, configured, running, and ready path.
///
/// Each state other than ready names the next step, as a command or, for a missing
/// program, as the plugin's own install help. Clients run when the user opens them, so the
/// daemon never reports them as failed.
pub(crate) fn plugin_state(
    id: &str,
    entry: &Entry,
    configured: bool,
    observed: &Observed,
    service: bool,
) -> serde_json::Value {
    let client = entry.capabilities.iter().all(|capability| capability == "client");
    let loaded = observed.active.is_some_and(|ids| ids.iter().any(|item| item == id));
    let mut reason = None;

    // Status only checks credentials of plugins that did not load, so `configured` is false
    // for loaded ones; a missing program matters once the plugin is set up either way.
    let (state, next) = if let Some(help) = observed.missing.filter(|_| loaded || configured) {
        ("missing program", Some(help.to_owned()))
    } else if loaded || (client && configured) {
        ("ready", None)
    } else if !configured {
        ("needs setup", Some(setup_command(id)))
    } else if observed.active.is_none() {
        let start = if service { "krabb service start" } else { "krabb service install" };

        ("configured", Some(start.to_owned()))
    } else {
        reason = observed.failure.map(|failure| failure.reason.as_str());

        let next = if observed.failure.is_some_and(|failure| failure.setup) {
            format!("krabb plugin setup {id}")
        } else {
            "krabb service restart".to_owned()
        };

        ("failed to start", Some(next))
    };

    serde_json::json!({"id": id, "state": state, "reason": reason, "next": next})
}

/// One plugin list item with its runtime state, as the CLI and the TUI show it.
///
/// A missing binary always wins, and a plugin status does not know, such as a folder
/// outside the lock, is only `installed`.
pub(crate) fn listed(mut item: Value, states: &[Value], binary: bool) -> Value {
    let state = states.iter().find(|state| state["id"] == item["id"]);

    item["health"] = match (binary, state) {
        (false, _) => "missing".into(),
        (true, Some(state)) => state["state"].clone(),
        (true, None) => "installed".into(),
    };

    if let Some(state) = state.filter(|_| binary) {
        item["reason"] = state["reason"].clone();
        item["next"] = state["next"].clone();
    }

    item
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

/// The status as the plugin tree layout: a headline, one branch per part, and one next action.
pub(crate) fn status_text(value: &serde_json::Value, installed: usize) -> String {
    let version = value["version"].as_str().unwrap_or(VERSION);
    let healthy = value["health"] == "healthy";
    let headline = if healthy { "is healthy" } else { "needs attention" };

    let plugins = value["plugin_states"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|plugin| {
            let line = format!(
                "{}: {}",
                plugin["id"].as_str().unwrap_or_default(),
                plugin["state"].as_str().unwrap_or_default()
            );

            match plugin["reason"].as_str() {
                Some(reason) => format!("{line} ({})", reason.trim_end_matches('.')),
                None => line,
            }
        })
        .collect::<Vec<_>>();

    let details = value["health_details"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(|detail| advice(detail).0)
        .collect::<Vec<_>>();

    let mut parts = vec![
        (format!("Runtime: {}", value["daemon"].as_str().unwrap_or("stopped")), Vec::new()),
        (format!("Intelligence: {}", capability_text(&value["intelligence"])), Vec::new()),
        (format!("Channels: {}", capability_text(&value["channels"])), Vec::new()),
        (format!("Plugins: {installed} installed"), plugins),
    ];

    if let Some(from) = value["recovery"]["from"].as_str() {
        let state =
            if value["recovery"]["accepted"] == true { "kept" } else { "update unfinished" };

        parts.push((format!("Recovery point: Krabb {from} ({state})"), Vec::new()));
    }

    if let Some(recovered) = recovered(value) {
        let line = if recovered.finished() {
            format!("Interrupted update: finished, Krabb {} kept", recovered.to)
        } else if recovered.restored {
            format!("Interrupted update: undone, Krabb {} restored", recovered.from)
        } else {
            format!("Interrupted update: stopped before any change, Krabb {} kept", recovered.kept)
        };

        parts.push((line, Vec::new()));
    }

    if !details.is_empty() {
        parts.push(("Needs attention".into(), details));
    }

    let mut output = format!("Krabb {version} {headline}\n{}", format_tree(&parts));

    if let Some(next) = next_action(value) {
        output.push_str(&format!("\n\n{next}"));
    }

    output
}

/// The one command that moves the installation forward, as a sentence.
pub(crate) fn next_action(value: &serde_json::Value) -> Option<String> {
    if value["recovery"]["from"].is_string() && value["recovery"]["accepted"] != true {
        return Some("Undo the unfinished update with `krabb update --rollback`.".into());
    }

    let plugins = value["plugin_states"].as_array().into_iter().flatten();

    // A plugin that did not load also makes its capability "not ready", so its cause comes first.
    if let Some(plugin) = plugins.clone().find(|plugin| plugin["state"] == "failed to start") {
        let id = plugin["id"].as_str().unwrap_or_default();

        return Some(match plugin["next"].as_str() {
            Some(next) if next.starts_with("krabb plugin setup") => {
                format!("Set up {id} again with `{next}`.")
            }

            _ => restart(id),
        });
    }

    if let Some(help) = plugins
        .clone()
        .find(|plugin| plugin["state"] == "missing program")
        .and_then(|plugin| plugin["next"].as_str())
    {
        return Some(help.to_owned());
    }

    let details = value["health_details"].as_array().into_iter().flatten();

    if let Some(next) =
        details.filter_map(serde_json::Value::as_str).find_map(|detail| advice(detail).1)
    {
        return Some(next.into());
    }

    let plugin = plugins.into_iter().find_map(|plugin| {
        let id = plugin["id"].as_str().unwrap_or_default();
        let next = plugin["next"].as_str()?;

        Some(match (plugin["state"].as_str(), next) {
            (Some("needs setup"), _) => format!("Set up {id} with `{next}`."),

            (Some("configured"), "krabb service install") => {
                "Install the service with `krabb service install`.".into()
            }

            (Some("configured"), _) => format!("Start {id} with `{next}`."),
            _ => restart(id),
        })
    });

    // An undone update is the last thing to do, once everything else works.
    plugin.or_else(|| {
        recovered(value)
            .filter(|recovered| !recovered.finished())
            .map(|_| "Try the update again with `krabb update`.".to_owned())
    })
}

fn recovered(value: &serde_json::Value) -> Option<upgrade::Outcome> {
    serde_json::from_value(value["recovered"].clone()).ok()
}

fn restart(id: &str) -> String {
    format!(
        "The log at {} shows why {id} failed.\nRestart {id} with `krabb service restart`.",
        logging::application_log_path().display()
    )
}

/// How the text output names a health detail, and the one command that resolves it.
fn advice(detail: &str) -> (String, Option<&'static str>) {
    let (problem, next) = match detail {
        "configuration is missing (run `krabb init`)" => {
            ("Configuration is missing", "Create the configuration with `krabb init`.")
        }

        "configuration is invalid" => {
            ("Configuration is invalid", "Find the invalid setting with `krabb doctor`.")
        }

        "no intelligence plugin is installed (run `krabb plugin install <provider>`)" => (
            "No intelligence plugin is installed",
            "Install a provider with `krabb plugin install <provider>`.",
        ),

        "selected intelligence plugin is not ready" => (
            "The selected intelligence plugin is not ready",
            "Set it up with `krabb init provider`.",
        ),

        "no channel is ready or the selected channel is not ready" => {
            ("No channel is ready", "Set one up with `krabb init channel`.")
        }

        "plugin commands have name conflicts" => (
            "Plugin commands have name conflicts",
            "Uninstall one of the conflicting plugins with `krabb plugin uninstall <id>`.",
        ),

        detail => return (sentence(detail).trim_end_matches('.').to_owned(), None),
    };

    (problem.into(), Some(next))
}
