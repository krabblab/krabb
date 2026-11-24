//! Formats plugin, session, delivery, and tree lists for the CLI.

use crate::*;

pub(crate) fn plugin_summary(manifest: &Manifest, health: &str) -> Value {
    serde_json::json!({
        "id": manifest.id,
        "version": manifest.version,
        "protocol": manifest.protocol,
        "capabilities": manifest.capabilities,
        "permissions": manifest.permissions,
        "secrets": manifest.variables,
        "settings": manifest.settings,
        "commands": manifest.commands,
        "setup_command": (!manifest.variables.is_empty() || !manifest.settings.is_empty())
            .then(|| format!("krabb plugin setup {}", manifest.id)),
        "status": "installed",
        "health": health,
    })
}

pub(crate) fn format_plugin_list(items: &[Value], page: usize) -> Result<String, String> {
    if items.is_empty() {
        return Ok(NO_PLUGINS.into());
    }

    let mut items = items.iter().collect::<Vec<_>>();

    items.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));

    let (start, end, pages) = page_bounds(page, items.len(), "plugins")?;

    let mut output =
        format!("Plugins ({}–{} of {}, page {page} of {pages})", start + 1, end, items.len());

    for (index, item) in items[start..end].iter().enumerate() {
        let id = item["id"].as_str().unwrap_or("unknown");
        let version = item["version"].as_str().unwrap_or("unknown");
        let health = item["health"].as_str().unwrap_or("unknown");
        let capabilities = string_list(&item["capabilities"]);
        let permissions = string_list(&item["permissions"]);

        let commands = item["commands"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|command| command["name"].as_str())
            .collect::<Vec<_>>();

        let protocol = format!(
            "{}.{}",
            item["protocol"]["major"].as_u64().unwrap_or_default(),
            item["protocol"]["minor"].as_u64().unwrap_or_default()
        );

        let commands = if commands.is_empty() { "none".into() } else { commands.join(", ") };

        let capabilities = if capabilities == "none" {
            "No capabilities".into()
        } else {
            format!("Capabilities include {capabilities}")
        };

        let commands = if commands == "none" {
            "No commands".into()
        } else {
            format!("Commands include {commands}")
        };

        let permissions = if permissions == "none" {
            "No permissions".into()
        } else {
            format!("Permissions include {permissions}")
        };

        let binary = match health {
            "ready" => "Binary is present (credentials not checked)".into(),
            "missing" => "Binary is missing, so reinstall the plugin from its source".into(),
            _ => format!("Health is {health}"),
        };

        let mut details =
            vec![binary, capabilities, format!("Uses protocol version {protocol}"), commands];

        if let Some(command) = item["setup_command"].as_str() {
            details.push(format!("Set up credentials with `{command}`"));
        }

        details.push(permissions);

        let separator = if index == 0 { "\n" } else { "\n\n" };

        output.push_str(&format!(
            "{separator}{}",
            format_tree_entry(&format!("{id}@v{version}"), &details, index + 1 == end - start)
        ));
    }

    Ok(output)
}

/// The empty plugin list, which ends with the command that adds the first plugin.
pub(crate) const NO_PLUGINS: &str =
    "No plugins installed.\nInstall one with `krabb plugin install <id>`.";

/// The empty delivery list, which needs nothing more.
const NO_DELIVERIES: &str = "No pending deliveries.\nEvery reply has been delivered.";

/// The sentence that ends every empty plugin result.
pub(crate) const INSTALL_ONE: &str = "Install one with `krabb plugin install <id>`.";

/// The empty session list, which ends with the command that starts a conversation.
const NO_SESSIONS: &str = "No sessions yet.\nStart a conversation with `krabb tui`.";

pub(crate) fn format_session_list(value: &Value, page: usize) -> Result<String, String> {
    let Some(items) = value["items"].as_array() else {
        return Err("The session list returned an invalid response.".into());
    };

    if items.is_empty() {
        return Ok(NO_SESSIONS.into());
    }

    let mut items = items.iter().filter(|item| item["id"].as_str().is_some()).collect::<Vec<_>>();

    items.sort_by(|left, right| {
        session_list_priority(left)
            .cmp(&session_list_priority(right))
            .then_with(|| {
                right["updated"]
                    .as_u64()
                    .unwrap_or_default()
                    .cmp(&left["updated"].as_u64().unwrap_or_default())
            })
            .then_with(|| left["id"].as_str().cmp(&right["id"].as_str()))
    });

    if items.is_empty() {
        return Ok(NO_SESSIONS.into());
    }

    let total = items.len();
    let (start, end, pages) = page_bounds(page, total, "sessions")?;
    let mut output = format!("Sessions ({}–{} of {total}, page {page} of {pages})", start + 1, end);

    for (index, item) in items[start..end].iter().enumerate() {
        let id = item["id"].as_str().unwrap_or("unknown");

        let status = if item["archived"] == true {
            "archived"
        } else if item["status"] == "working" || item["inflight"] == true {
            "working"
        } else {
            item["status"].as_str().unwrap_or("unknown")
        };

        let model = item["model"].as_str().filter(|model| !model.is_empty()).unwrap_or("unset");
        let mut details = vec![format!("Uses model {model}")];

        if item["channel"].is_string() && item["pinned"] == false {
            details.push("Follows the channel's model setting".into());
        }

        if let Some(created) = item["created"].as_u64().and_then(format_date) {
            details.push(format!("Created on {created}"));
        }

        if let Some(updated) = item["updated"].as_u64().and_then(format_date) {
            details.push(format!("Updated on {updated}"));
        }

        if let Some(messages) = item["messages"].as_u64() {
            details.push(format!("Has {messages} messages"));
        }

        let separator = if index == 0 { "\n" } else { "\n\n" };

        output.push_str(&format!(
            "{separator}{}",
            format_tree_entry(&format!("{id} | {status}"), &details, index + 1 == end - start)
        ));
    }

    Ok(output)
}

pub(crate) fn format_delivery_list(value: &Value, page: usize) -> Result<String, String> {
    let Some(items) = value["items"].as_array() else {
        return Err("The delivery list returned an invalid response.".into());
    };

    if items.is_empty() {
        return Ok(NO_DELIVERIES.into());
    }

    let items = items.iter().filter(|item| item["id"].as_str().is_some()).collect::<Vec<_>>();

    if items.is_empty() {
        return Ok(NO_DELIVERIES.into());
    }

    let total = items.len();
    let (start, end, pages) = page_bounds(page, total, "deliveries")?;

    let mut output =
        format!("Deliveries ({}–{} of {total}, page {page} of {pages})", start + 1, end);

    for (index, item) in items[start..end].iter().enumerate() {
        let id = item["id"].as_str().unwrap_or("unknown");
        let status = item["status"].as_str().unwrap_or("unknown");
        let mut details = Vec::new();

        if let Some(channel) = item["channel"].as_str() {
            details.push(format!("Sent through {channel}"));
        }

        if let Some(attempts) = item["attempts"].as_u64() {
            details.push(format!("Attempted {attempts} times"));
        }

        if let Some(error) = item["error"].as_str().filter(|error| !error.is_empty()) {
            details.push(format!("Last attempt failed. {}", sentence(error.to_owned())));
        }

        let separator = if index == 0 { "\n" } else { "\n\n" };

        output.push_str(&format!(
            "{separator}{}",
            format_tree_entry(&format!("{id} | {status}"), &details, index + 1 == end - start)
        ));
    }

    Ok(output)
}

pub(crate) fn page_bounds(
    page: usize,
    total: usize,
    kind: &str,
) -> Result<(usize, usize, usize), String> {
    validate_page(page)?;

    let pages = total.div_ceil(CLI_LIST_PAGE_SIZE);

    if page > pages {
        return Err(format!(
            "Page {page} is out of range for {kind}, which has pages 1 to {pages}."
        ));
    }

    let start = (page - 1) * CLI_LIST_PAGE_SIZE;
    let end = (start + CLI_LIST_PAGE_SIZE).min(total);

    Ok((start, end, pages))
}

pub(crate) fn validate_page(page: usize) -> Result<(), String> {
    if page == 0 { Err("Page numbers start at 1.".into()) } else { Ok(()) }
}

pub(crate) fn session_list_priority(item: &Value) -> u8 {
    if item["status"] == "working" || item["inflight"] == true {
        0
    } else if item["archived"] == true {
        3
    } else {
        2
    }
}

pub(crate) fn format_tree_entry(label: &str, details: &[String], is_last: bool) -> String {
    let root_branch = if is_last { "└─" } else { "├─" };

    let child_prefix = if is_last { "   " } else { "│  " };

    let mut lines = vec![format!("{root_branch} {label}")];

    lines.extend(details.iter().enumerate().map(|(index, detail)| {
        let branch = if index + 1 == details.len() { "└─" } else { "├─" };

        format!("{child_prefix}{branch} {detail}")
    }));

    lines.join("\n")
}

pub(crate) fn format_tree_list(entries: &[String]) -> String {
    let entries = entries.iter().map(|entry| (entry.clone(), Vec::new())).collect::<Vec<_>>();

    format_tree(&entries)
}

/// One tree of labeled entries, each with its own details, as plugin results print them.
pub(crate) fn format_tree(entries: &[(String, Vec<String>)]) -> String {
    entries
        .iter()
        .enumerate()
        .map(|(index, (label, details))| {
            format_tree_entry(label, details, index + 1 == entries.len())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn format_date(timestamp: u64) -> Option<String> {
    const SECONDS_PER_DAY: u64 = 86_400;

    const DAYS_BEFORE_UNIX_EPOCH: i64 = 719_468;
    const DAYS_PER_ERA: i64 = 146_097;

    let days = i64::try_from(timestamp / SECONDS_PER_DAY).ok()? + DAYS_BEFORE_UNIX_EPOCH;
    let era = days / DAYS_PER_ERA;
    let day_of_era = days - era * DAYS_PER_ERA;

    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;

    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let month = march_month + if march_month < 10 { 3 } else { -9 };

    if month <= 2 {
        year += 1;
    }

    Some(format!("{year:04}-{month:02}-{day:02}"))
}
