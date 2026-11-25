//! The plugin list screen that the CLI and the TUI share.

use serde_json::Value;

use crate::tree::format_tree_entry;

pub fn format_plugin_list(items: &[Value], page: usize, size: usize) -> Result<String, String> {
    if items.is_empty() {
        return Ok(NO_PLUGINS.into());
    }

    let mut items = items.iter().collect::<Vec<_>>();

    items.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));

    let (start, end, pages) = page_bounds(page, items.len(), size, "plugins")?;

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

        let reason = item["reason"].as_str().map(|reason| reason.trim_end_matches('.'));
        let next = item["next"].as_str().map(|next| next.trim_end_matches('.'));

        let state = match (health, reason, next) {
            ("ready", ..) => "Ready".into(),
            ("missing", ..) => "Binary is missing, so reinstall the plugin from its source".into(),
            ("needs setup", ..) => "Needs setup".into(),
            ("configured", ..) => "Configured, but the runtime is not running".into(),
            ("failed to start", Some(reason), _) => format!("Failed to start: {reason}"),
            ("failed to start", None, _) => "Failed to start".into(),
            ("missing program", _, Some(help)) => format!("Missing a program it runs: {help}"),
            _ => format!("Health is {health}"),
        };

        let mut details =
            vec![state, capabilities, format!("Uses protocol version {protocol}"), commands];

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
pub const NO_PLUGINS: &str = "No plugins installed.\nInstall one with `krabb plugin install <id>`.";

/// The first and last item on PAGE and the page count, or why the page does not exist.
pub fn page_bounds(
    page: usize,
    total: usize,
    size: usize,
    kind: &str,
) -> Result<(usize, usize, usize), String> {
    validate_page(page)?;

    let pages = total.div_ceil(size);

    if page > pages {
        return Err(format!(
            "Page {page} is out of range for {kind}, which has pages 1 to {pages}."
        ));
    }

    let start = (page - 1) * size;
    let end = (start + size).min(total);

    Ok((start, end, pages))
}

pub fn validate_page(page: usize) -> Result<(), String> {
    if page == 0 { Err("Page numbers start at 1.".into()) } else { Ok(()) }
}

fn string_list(value: &Value) -> String {
    let values =
        value.as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>();

    if values.is_empty() { "none".into() } else { values.join(", ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_start_at_one_and_stop_at_the_last() {
        assert_eq!(page_bounds(1, 7, 5, "plugins"), Ok((0, 5, 2)));
        assert_eq!(page_bounds(2, 7, 5, "plugins"), Ok((5, 7, 2)));
        assert_eq!(page_bounds(0, 7, 5, "plugins"), Err("Page numbers start at 1.".into()));

        assert_eq!(
            page_bounds(3, 7, 5, "plugins"),
            Err("Page 3 is out of range for plugins, which has pages 1 to 2.".into())
        );
    }

    #[test]
    fn plugins_without_details_say_so() {
        let items = [serde_json::json!({"id": "echo", "version": "0.1.0", "health": "missing"})];

        assert_eq!(
            format_plugin_list(&items, 1, 5).unwrap(),
            "Plugins (1–1 of 1, page 1 of 1)\n└─ echo@v0.1.0\n   ├─ Binary is missing, so \
             reinstall the plugin from its source\n   ├─ No capabilities\n   ├─ Uses protocol \
             version 0.0\n   ├─ No commands\n   └─ No permissions"
        );

        assert_eq!(format_plugin_list(&[], 1, 5).unwrap(), NO_PLUGINS);
    }
}
