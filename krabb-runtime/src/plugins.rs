//! Implements the `krabb plugin` command family and its output.

use crate::*;

pub(crate) async fn plugin(
    command: PluginCommand,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match command {
        PluginCommand::Setup(options) => {
            setup::run(options, &home(), json).await?;
        }

        PluginCommand::List(output) => {
            let root = home();

            // A daemon that cannot answer leaves the list to what this process can check.
            let runtime = runtime_at(&root).await.ok().flatten();

            list_at(&root, runtime, output.page, output.json || json)?;
        }
        PluginCommand::Detach(args) => lifecycle::run(&home(), &args.id, false, json).await?,
        PluginCommand::Attach(args) => lifecycle::run(&home(), &args.id, true, json).await?,

        PluginCommand::Install(install) => {
            let unstable = install.unstable;
            let quiet = install.quiet;
            let (linked, mut sources) = plugin_sources(install)?;
            let working_dir = std::env::current_dir()?;

            ensure_linkable_sources(linked, &sources, &working_dir)?;

            let root = home();

            let replacements = sources
                .iter()
                .filter(|source| root.join("plugins").join(&source.id).exists())
                .map(|source| source.id.as_str())
                .collect::<Vec<_>>();

            if !replacements.is_empty() {
                if !confirm_action(
                    sources.iter().all(|source| source.yes),
                    json,
                    &format!("Replace installed plugins {}?", replacements.join(", ")),
                )? {
                    print_cancelled("Install", json);
                    return Ok(());
                }

                for source in &mut sources {
                    source.yes = true;
                }
            }

            let mut installed = Vec::new();

            let mut errors = Vec::new();
            let mut activation_errors = Vec::new();
            let mut waiting_for_credentials = false;
            let mut outcomes = Vec::new();
            let action = if linked { "Linking" } else { "Installing" };

            let mut notices = Vec::new();

            for mut source in sources {
                let id = source.id.clone();
                let spinner = terminal::progress(format!("{action} plugin {id}..."), json);

                let pin = if unstable { index::unstable(&root, &id) } else { Ok(None) };

                if let Ok(Some(version)) = &pin {
                    source.version = Some(version.clone());
                }

                let pinned = source.version.clone();

                let existing_version =
                    read_manifest(&root.join("plugins").join(&id)).map(|manifest| manifest.version);

                let mut credentials_deferred = false;

                match pin.and_then(|_| link(source, linked)) {
                    Ok(manifest) => {
                        drop(spinner);

                        if let Some(notice) =
                            pinned.and_then(|version| index::notice(&root, &id, &version))
                        {
                            notices.push(notice);
                        }

                        let _progress =
                            terminal::progress(format!("Activating plugin {id}..."), json);

                        let state = match activate_at(&root, &manifest.id).await {
                            Ok(activation) => {
                                if activation.is_some() {
                                    "active"
                                } else {
                                    "deferred"
                                }
                            }

                            Err(error) if plugin_activation_is_deferred(&error.to_string()) => {
                                debug!(plugin = %id, "Plugin installed, and activation is deferred.");
                                waiting_for_credentials = true;
                                credentials_deferred = true;
                                "deferred"
                            }

                            Err(error) => {
                                warn!(plugin = %id, error = %diagnostic(sentence(error.to_string())), "Plugin activation failed after installation.");
                                activation_errors.push(serde_json::json!({
                                    "id": id,
                                    "error": error.to_string()
                                }));
                                "failed"
                            }
                        };

                        if linked {
                            info!(plugin = %manifest.id, version = %manifest.version, activation = state, "Plugin linked.");
                        } else {
                            info!(plugin = %manifest.id, version = %manifest.version, activation = state, "Plugin installed.");
                        }

                        outcomes.push(serde_json::json!({
                            "id": manifest.id.clone(),
                            "version": manifest.version.clone(),
                            "status": if state == "failed" { "activation_failed" } else { "installed" },
                            "activation": state,
                            "waiting_for_credentials": credentials_deferred
                        }));

                        installed.push(serde_json::json!({
                            "id": manifest.id,
                            "version": manifest.version,
                            "activation": state,
                            "linked": linked
                        }));
                    }

                    Err(error) => {
                        warn!(plugin = %id, error = %diagnostic(sentence(error.to_string())), "Plugin installation failed.");
                        let next =
                            error.downcast_ref::<index::Unavailable>().map(|error| error.next());

                        errors.push(serde_json::json!({
                            "id": id,
                            "error": error.to_string(),
                            "next": next,
                        }));

                        outcomes.push(serde_json::json!({
                            "id": id,
                            "version": existing_version,
                            "status": "failed",
                            "error": error.to_string(),
                            "next": next,
                        }));
                    }
                }
            }

            if json {
                let failed = errors.len();
                let total = installed.len() + failed;

                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "action": "install",
                        "items": &installed,
                        "notices": &notices,
                        "errors": &errors,
                        "activation_errors": &activation_errors
                    }))?
                );

                if failed > 0 {
                    return Err(
                        format!("{failed} of {total} plugin installation(s) failed.").into()
                    );
                }
            } else {
                let total = installed.len() + errors.len();
                let failed = !errors.is_empty() || !activation_errors.is_empty();

                if !quiet || failed {
                    println!("{}", plugin_install_output(&outcomes, linked));
                }

                for notice in &notices {
                    println!("\n{notice}");
                }

                if !errors.is_empty() {
                    println!();
                    std::io::stdout().flush()?;
                    return Err(plugin_install_error().into());
                }

                if !activation_errors.is_empty() {
                    for failure in &activation_errors {
                        println!(
                            "\n{}: {}",
                            failure["id"].as_str().unwrap_or("plugin"),
                            diagnostic(failure["error"].as_str().unwrap_or("Activation failed."))
                        );
                    }
                } else if installed.len() == total && !waiting_for_credentials && !quiet {
                    println!("\n{}", plugin_service_guidance(service_path().is_file()));
                }
            }

            if !activation_errors.is_empty() {
                let restart = if service_path().is_file() {
                    "Restart them with `krabb service restart`."
                } else {
                    "Load them by restarting `krabb-daemon`."
                };

                return Err(format!(
                    "Plugins were installed, but activation failed, as shown above.\nThe log \
                     at {} has more detail.\n{restart}",
                    logging::application_log_path().display()
                )
                .into());
            }
        }

        PluginCommand::Update(update) => update_plugins(update.yes, update.json || json).await?,
        PluginCommand::Versions(args) => index::versions(&home(), &args.id, json)?,

        PluginCommand::Remove(name) => {
            let ids = plugin_uninstall_ids(name.ids)?;

            let mut prompt = if ids.len() == 1 {
                format!("Uninstall plugin {}?", ids[0])
            } else {
                format!("Uninstall plugins {}?", ids.join(", "))
            };

            if name.force {
                prompt.push_str(
                    " --force purges session/delivery state for the last provider/channel.",
                );
            }

            if !confirm_action(name.yes, json, &prompt)? {
                print_cancelled("Uninstall", json);
                return Ok(());
            }

            let root = home();

            let mut outcomes = Vec::new();
            let mut errors = Vec::new();
            let mut display_rows = Vec::new();

            for id in &ids {
                let _progress = terminal::progress(format!("Uninstalling plugin {id}..."), json);

                match uninstall_plugin_at(&root, id, name.force).await {
                    Ok(outcome) => {
                        let version = outcome["version"]
                            .as_str()
                            .map(|version| format!("@v{version}"))
                            .unwrap_or_default();

                        display_rows.push(format!("{id}{version} successfully uninstalled"));
                        outcomes.push(outcome);
                    }

                    Err(error) => {
                        warn!(plugin = %id, error = %diagnostic(sentence(error.to_string())), "Plugin uninstallation failed.");

                        let version = load_lock_at(&root)
                            .ok()
                            .and_then(|lock| {
                                lock.plugins.get(id).map(|entry| entry.version.clone())
                            })
                            .map(|version| format!("@v{version}"))
                            .unwrap_or_default();

                        display_rows.push(format!("{id}{version} uninstallation failed"));

                        errors.push(serde_json::json!({
                            "id": id,
                            "error": sentence(error.to_string())
                        }));
                    }
                }
            }

            if json {
                if ids.len() == 1 {
                    if let Some(error) = errors.first() {
                        return Err(error["error"]
                            .as_str()
                            .unwrap_or("Plugin uninstallation failed.")
                            .into());
                    }

                    let outcome = &outcomes[0];

                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "action": "uninstall",
                            "id": outcome["id"],
                            "status": "uninstalled",
                            "unloaded": outcome["unloaded"],
                            "purged": outcome["purged"],
                        }))?
                    );
                } else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "action": "uninstall",
                            "items": outcomes,
                            "errors": errors,
                        }))?
                    );

                    if !errors.is_empty() {
                        return Err(plugin_uninstall_error(errors.len(), ids.len()).into());
                    }
                }
            } else if ids.len() == 1 {
                if let Some(error) = errors.first() {
                    return Err(error["error"]
                        .as_str()
                        .unwrap_or("Plugin uninstallation failed.")
                        .into());
                }

                println!("{}", plugin_uninstall_output(&outcomes[0]));
            } else {
                println!("{}", plugin_uninstall_batch_output(&display_rows, ids.len()));

                if !errors.is_empty() {
                    std::io::stdout().flush()?;
                    return Err(plugin_uninstall_error(errors.len(), ids.len()).into());
                }
            }
        }
    }

    Ok(())
}

pub(crate) fn plugin_sources(
    install: PluginInstall,
) -> Result<(bool, Vec<Source>), Box<dyn std::error::Error + Send + Sync>> {
    if install.ids.is_empty() {
        return Err("At least one plugin ID is required.".into());
    }

    if (install.source.is_some() || install.revision.is_some()) && install.ids.len() != 1 {
        return Err("--source and --revision can be used with only one plugin ID.".into());
    }

    if install.revision.is_some() && install.source.is_none() {
        return Err("--revision requires --source.".into());
    }

    if install.unstable && (install.source.is_some() || install.link) {
        return Err("--unstable cannot be combined with --source or --link.".into());
    }

    let mut seen = std::collections::BTreeSet::new();
    let mut sources = Vec::new();

    for value in install.ids {
        let (id, version) = value
            .split_once('@')
            .map_or((value.as_str(), None), |(id, version)| (id, Some(version.to_owned())));

        if !valid(id) {
            return Err(format!("Invalid plugin ID: {id}.").into());
        }

        if !seen.insert(id.to_owned()) {
            return Err(format!("Plugin ID {id} was listed more than once.").into());
        }

        if let Some(version) = &version {
            if install.unstable {
                return Err("--unstable cannot be combined with a plugin version.".into());
            }

            if install.source.is_some() || install.link {
                return Err("Plugin versions cannot be combined with --source or --link.".into());
            }

            if version != "latest" && !releases::valid_version(version) {
                return Err(format!("Version {version} is not a valid plugin version.").into());
            }
        }

        let version = version.filter(|version| version != "latest");

        sources.push(Source {
            id: id.to_owned(),
            source: install.source.clone(),
            revision: install.revision.clone(),
            version,
            yes: install.yes,
        });
    }

    Ok((install.link, sources))
}

pub(crate) fn plugin_install_output(outcomes: &[serde_json::Value], linked: bool) -> String {
    let total = outcomes.len();
    let failed = outcomes.iter().filter(|item| item["status"] == "failed").count();
    let installed = total - failed;
    let action = if linked { "Linked" } else { "Installed" };

    let heading = if failed == 0 {
        format!("{action} {installed} {}", if installed == 1 { "plugin" } else { "plugins" })
    } else {
        format!("{action} {installed} of {total} plugins")
    };

    let rows = outcomes
        .iter()
        .map(|item| {
            let id = item["id"].as_str().unwrap_or("unknown");
            let version = item["version"].as_str().map(|version| format!("@v{version}"));
            let label = format!("{}{}", id, version.as_deref().unwrap_or(""));
            let result = match (item["status"].as_str(), item["activation"].as_str()) {
                (Some("failed"), _) if linked => "link failed",
                (Some("failed"), _) => "installation failed",
                (Some("activation_failed"), _) => "installed, but activation failed",

                (_, _) if item["waiting_for_credentials"] == true => {
                    "installed and waiting for credentials"
                }

                (_, Some("deferred")) => "installed, with activation deferred",
                _ if linked => "successfully linked",
                _ => "successfully installed",
            };

            let mut reason = item["error"]
                .as_str()
                .map(|error| {
                    terminal::printable(&diagnostic(error))
                        .lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty())
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            reason.extend(item["next"].as_str().map(str::to_owned));

            (format!("{label} {result}"), reason)
        })
        .collect::<Vec<_>>();

    let mut output = heading;

    if !rows.is_empty() {
        output.push('\n');
        output.push_str(&format_tree(&rows));
    }

    output
}

/// The line that ends a failed install, whose rows already name each reason and command.
pub(crate) fn plugin_install_error() -> String {
    format!("The log at {} has more detail.", logging::application_log_path().display())
}

pub(crate) fn plugin_log_hint(failure: &str) -> String {
    format!(
        "For details about the {failure}, check the application logs:\n<{}>",
        logging::application_log_path().display()
    )
}

pub(crate) fn plugin_service_guidance(installed: bool) -> &'static str {
    if installed {
        "Activate the new plugins with `krabb service restart`."
    } else {
        "The Krabb service is not installed.\nRun every plugin in the background with `krabb \
         service install`."
    }
}

pub(crate) fn plugin_uninstall_ids(
    ids: Vec<String>,
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    if ids.is_empty() {
        return Err("At least one plugin ID is required.".into());
    }

    let mut seen = std::collections::BTreeSet::new();

    for id in &ids {
        if !valid(id) {
            return Err(format!("Invalid plugin ID: {id}.").into());
        }

        if !seen.insert(id) {
            return Err(format!("Plugin ID {id} was listed more than once.").into());
        }
    }

    Ok(ids)
}

pub(crate) async fn uninstall_plugin_at(
    home_root: &Path,
    id: &str,
    force: bool,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(result) =
        control_at("plugin.uninstall", serde_json::json!({"id": id, "force": force}), home_root)
            .await?
    {
        return Ok(result);
    }

    let _offline = offline_lock(home_root)?;
    let lock = load_lock_at(home_root)?;
    let version = lock.plugins.get(id).map(|entry| entry.version.clone());
    let (purge_sessions, purge_deliveries) = last_plugin_state(&lock, id, home_root);

    let purged = if force && (purge_sessions || purge_deliveries) {
        Some(purge_offline_state(home_root, purge_sessions, purge_deliveries)?)
    } else {
        None
    };

    let revoked = remove_guarded(PluginRemove { id: id.into(), yes: true }, home_root, |lock| {
        ensure_no_orphaned_plugin_state(lock, id, home_root)
    })?;

    Ok(serde_json::json!({
        "id": id, "version": version, "status": "uninstalled", "unloaded": false, "purged": purged,
        "revoked": revoked,
    }))
}

pub(crate) fn check_plugin_state(
    lock: &Lock,
    id: &str,
    root: &Path,
    state: &Value,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (model, channel) = last_plugin_state(lock, id, root);

    let sessions = state["sessions"].as_bool().ok_or("Invalid plugin state guard response.")?;
    let deliveries = state["deliveries"].as_bool().ok_or("Invalid plugin state guard response.")?;

    if model && sessions {
        return Err(concat!(
            "Cannot uninstall the last intelligence plugin while sessions exist. ",
            "Delete the sessions with `krabb session delete` first, or use ",
            "`krabb plugin uninstall <id> --force` to delete blocking state."
        )
        .into());
    }

    if channel && deliveries {
        return Err(concat!(
            "Cannot uninstall the last channel plugin while deliveries are pending or ",
            "uncertain. Use `krabb delivery drop --all`, or ",
            "`krabb plugin uninstall <id> --force` to delete blocking delivery records."
        )
        .into());
    }

    Ok(())
}

pub(crate) fn plugin_uninstall_output(outcome: &Value) -> String {
    let id = outcome["id"].as_str().unwrap_or("unknown");
    let mut output = format!("Uninstalled plugin {id}.");
    let mut details = Vec::new();

    if outcome["unloaded"] == true {
        details.push("Plugin unloaded from the background runtime.".to_owned());
    }

    if let Some(purged) = outcome["purged"].as_object() {
        let sessions = purged.get("sessions").and_then(Value::as_u64).unwrap_or_default();
        let deliveries = purged.get("deliveries").and_then(Value::as_u64).unwrap_or_default();

        if sessions > 0 || deliveries > 0 {
            let session_noun = if sessions == 1 { "session" } else { "sessions" };

            let delivery_noun =
                if deliveries == 1 { "delivery record" } else { "delivery records" };

            details.push(format!(
                "Removed {sessions} {session_noun} and {deliveries} {delivery_noun}."
            ));
        }

        if let Some(pending) = purged.get("pending_worktrees").and_then(Value::as_array)
            && !pending.is_empty()
        {
            let count = pending.len();
            let noun = if count == 1 { "session" } else { "sessions" };

            details.push(format!("Workspace cleanup is pending for {count} {noun}."));
        }
    }

    let revoked = &outcome["revoked"];

    let secrets = revoked["secrets"].as_array().map_or(0, Vec::len);
    let grants = revoked["grants"].as_u64().unwrap_or_default();

    if secrets > 0 || revoked["keyring"] == true {
        let noun = if secrets == 1 { "value" } else { "values" };

        details.push(format!("Removed {secrets} saved secret or setting {noun}."));
    }

    if grants > 0 {
        let noun = if grants == 1 { "grant" } else { "grants" };

        details.push(format!("Revoked {grants} sharing {noun}."));
    }

    if !details.is_empty() {
        output.push('\n');

        output.push_str(&format_tree_list(&details));
    }

    output
}

pub(crate) fn plugin_uninstall_batch_output(rows: &[String], total: usize) -> String {
    let successful = rows.iter().filter(|row| row.ends_with("successfully uninstalled")).count();

    let heading = if successful == total {
        format!("Uninstalled {successful} {}.", if successful == 1 { "plugin" } else { "plugins" })
    } else {
        format!("Uninstalled {successful} of {total} plugins.")
    };

    let mut output = heading;

    if !rows.is_empty() {
        output.push('\n');
        output.push_str(&format_tree_list(rows));
    }

    if successful < total {
        output.push('\n');
    }

    output
}

pub(crate) fn plugin_uninstall_error(failed: usize, total: usize) -> String {
    format!(
        "Failed to uninstall {failed} of {total} plugins.\n{}",
        plugin_log_hint("failed uninstallation")
    )
}

pub(crate) fn plugin_activation_is_deferred(error: &str) -> bool {
    error.contains("Channel credentials are not ready.")
}

pub(crate) fn plugin_update_error(rows: &[serde_json::Value]) -> String {
    let failures = rows.iter().filter(|row| row["status"] == "failed").collect::<Vec<_>>();

    let count = failures.len();
    let noun = if count == 1 { "plugin" } else { "plugins" };

    let mut message = format!("Could not update {count} {noun}.");

    for failure in failures {
        let id = failure["id"].as_str().unwrap_or("unknown");
        let detail = sentence(failure["error"].as_str().unwrap_or("Unknown error."));

        message.push_str(&format!("\n\nThe {id} plugin could not be updated. {detail}"));
    }

    message.push_str(&format!("\n\n{}", plugin_log_hint("failed update")));

    message
}

pub(crate) fn plugin_update_output(rows: &[serde_json::Value]) -> String {
    if rows.is_empty() {
        return format!("No plugins to update.\n{INSTALL_ONE}");
    }

    let updated = rows.iter().filter(|row| row["status"] == "updated").count();
    let failed = rows.iter().filter(|row| row["status"] == "failed").count();

    let heading = if failed == 0 && updated == 0 {
        "All plugins are up to date.".to_owned()
    } else if failed == 0 {
        format!("Updated {updated} {}.", if updated == 1 { "plugin" } else { "plugins" })
    } else {
        format!("Updated {updated} of {} plugins.", rows.len())
    };

    let entries = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let id = row["id"].as_str().unwrap_or("unknown");
            let version = row["version"].as_str().map(|version| format!("@v{version}"));
            let label = format!("{}{}", id, version.as_deref().unwrap_or(""));
            let result = match row["status"].as_str() {
                Some("updated") => "successfully updated",
                Some("failed") => "update failed",
                _ => "already up to date",
            };

            format_tree_entry(&format!("{label} {result}"), &[], index + 1 == rows.len())
        })
        .collect::<Vec<_>>();

    let mut output = heading;

    if !entries.is_empty() {
        output.push('\n');
        output.push_str(&entries.join("\n"));
    }

    output
}

pub(crate) async fn activate_at(
    root: &Path,
    id: &str,
) -> Result<Option<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> {
    control_at("plugin.load", serde_json::json!({"id": id}), root).await
}

pub(crate) async fn active_at(
    root: &Path,
) -> Result<Option<Vec<String>>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(runtime_at(root).await?.map(|runtime| runtime.active))
}

/// What a running daemon reports about its plugins, from its own environment.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Runtime {
    pub active: Vec<String>,
    /// Why each plugin the daemon could not start failed.
    pub failures: BTreeMap<String, String>,
    /// The help of a program each plugin needs that the daemon cannot find.
    pub missing: BTreeMap<String, String>,
}

impl Runtime {
    /// Adds the programs installed plugins need that this process cannot find on its PATH.
    pub fn with_missing(mut self, root: &Path) -> Self {
        if let Ok(lock) = load_lock_at(root) {
            self.missing = requirement::missing(root, &lock);
        }

        self
    }
}

pub(crate) async fn runtime_at(root: &Path) -> Result<Option<Runtime>, PluginError> {
    let Some(value) = control_at("plugin.active", serde_json::json!({}), root).await? else {
        return Ok(None);
    };

    Ok(Some(runtime(value)?))
}

/// Reads failures and missing programs leniently: an older daemon sends neither.
pub(crate) fn runtime(value: serde_json::Value) -> Result<Runtime, PluginError> {
    let reasons = |key: &str| {
        value[key]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(id, _)| valid(id))
            .filter_map(|(id, reason)| Some((id.clone(), start_failure(reason.as_str()?))))
            .collect::<BTreeMap<_, _>>()
    };

    let failures = reasons("failures");
    let missing = reasons("missing");

    Ok(Runtime { active: active_ids(value)?, failures, missing })
}

pub(crate) fn active_ids(value: serde_json::Value) -> Result<Vec<String>, PluginError> {
    let items =
        value["items"].as_array().ok_or("Daemon returned an invalid active plugin list.")?;

    let mut active = Vec::with_capacity(items.len());

    for item in items {
        let id = item.as_str().ok_or("Daemon returned an invalid active plugin ID.")?;

        if !valid(id) {
            return Err("Daemon returned an invalid active plugin ID.".into());
        }

        active.push(id.to_owned());
    }

    Ok(active)
}
