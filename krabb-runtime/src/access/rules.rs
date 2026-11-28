//! Lasting "Always allow" rules: a tool and an exact command or path, in one conversation.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Elevation, Role};
use crate::{Config, PluginError};

/// How many rules a home keeps, so matching stays cheap and `config.toml` readable.
pub(crate) const LIMIT: usize = 256;

const TEXT_LIMIT: usize = 4096;

/// One saved rule. `scope` is the conversation it was given in, such as `telegram:42` or
/// `tui`; `command` matches a shell command exactly, and `path` matches a file tool's path
/// exactly or, ending in `/**`, everything below a folder.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Rule {
    pub id: String,
    pub scope: String,
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub saved_by: Role,
}

impl Rule {
    /// The rule for TOOL with ARGS in SCOPE, saved by ROLE, or none for a call a rule cannot
    /// describe exactly.
    pub(crate) fn new(
        scope: &str,
        tool: &str,
        args: &serde_json::Value,
        role: Role,
    ) -> Option<Self> {
        let (command, path) = match tool {
            "shell_run" => (Some(args["command"].as_str()?.to_owned()), None),
            "file_write" | "file_patch" => (None, Some(args["path"].as_str()?.to_owned())),
            _ => return None,
        };

        let long =
            command.iter().chain(&path).any(|text| text.is_empty() || text.len() > TEXT_LIMIT);

        if long || scope.is_empty() {
            return None;
        }

        let mut rule = Self {
            id: String::new(),
            scope: scope.into(),
            tool: tool.into(),
            command,
            path,
            saved_by: role,
        };

        rule.id = rule.digest();

        Some(rule)
    }

    /// A short ID derived from what the rule allows, so saving the same rule twice keeps one.
    fn digest(&self) -> String {
        let text = format!(
            "{}\0{}\0{}\0{}",
            self.scope,
            self.tool,
            self.command.as_deref().unwrap_or_default(),
            self.path.as_deref().unwrap_or_default()
        );

        crate::digest::sha256(text)[..8].to_owned()
    }

    /// Whether the rule allows TOOL with ARGS in SCOPE.
    pub(crate) fn allows(&self, scope: &str, tool: &str, args: &serde_json::Value) -> bool {
        if self.scope != scope || self.tool != tool {
            return false;
        }

        match (&self.command, &self.path) {
            (Some(command), None) => args["command"].as_str() == Some(command.as_str()),
            (None, Some(path)) => args["path"].as_str().is_some_and(|value| covers(path, value)),
            _ => false,
        }
    }
}

/// Whether PATTERN covers PATH: the same path, or a path below `folder/**` without `..`.
fn covers(pattern: &str, path: &str) -> bool {
    match pattern.strip_suffix("/**") {
        Some(folder) => {
            path.strip_prefix(folder).is_some_and(|rest| rest.starts_with('/'))
                && !path.split(['/', '\\']).any(|part| part == "..")
        }

        None => pattern == path,
    }
}

/// The conversation a rule belongs to: a client by its ID, a channel chat as `channel:chat`.
pub(crate) fn scope(source: &crate::sharing::Principal, chat: &str) -> String {
    match source {
        crate::sharing::Principal::Client(id) => id.clone(),
        crate::sharing::Principal::Channel(id) => format!("{id}:{chat}"),
        crate::sharing::Principal::Agent(id) => format!("agent:{id}"),
    }
}

/// Whether a saved rule lets TOOL with ARGS run unasked in SCOPE. A rule for an admin command
/// counts only while the role that saved it still has the level the command needs.
pub(crate) fn allowed(
    config: &Config,
    saved: &[Rule],
    scope: &str,
    tool: &str,
    args: &serde_json::Value,
) -> bool {
    saved.iter().any(|rule| {
        rule.allows(scope, tool, args)
            && super::needs(args["command"].as_str().unwrap_or_default())
                .is_none_or(|level| level <= elevation(config, rule.saved_by))
    })
}

/// Whether ROLE may save a rule for TOOL with ARGS: anyone who may approve it, except that a
/// rule for an admin command needs the owner, or a role whose own level covers the command.
pub(crate) fn may_save(config: &Config, role: Role, tool: &str, args: &serde_json::Value) -> bool {
    let level = super::needs(if tool == "shell_run" {
        args["command"].as_str().unwrap_or_default()
    } else {
        ""
    });

    level.is_none_or(|level| role == Role::Owner || level <= elevation(config, role))
}

fn elevation(config: &Config, role: Role) -> Elevation {
    config.roles.get(&role).and_then(|setting| setting.elevation).unwrap_or_else(|| {
        config
            .roles
            .get(&role)
            .and_then(|setting| setting.preset)
            .unwrap_or(role.preset())
            .elevation()
    })
}

/// Appends RULE to `config.toml` in ROOT, keeping one copy of an identical rule.
pub(crate) fn save(root: &Path, rule: &Rule) -> Result<(), PluginError> {
    edit(root, |rules| {
        if rules.iter().any(|table| table.get("id").and_then(|id| id.as_str()) == Some(&rule.id)) {
            return Ok(false);
        }

        if rules.len() >= LIMIT {
            return Err(format!("Krabb keeps at most {LIMIT} rules, so forget one first.").into());
        }

        let value = toml::Value::try_from(rule)?;
        let text = toml::to_string(&value)?;
        let table = text.parse::<toml_edit::DocumentMut>()?.as_table().clone();

        rules.push(table);

        Ok(true)
    })
}

/// Removes the rule with ID from `config.toml` in ROOT, and says whether one was there.
pub(crate) fn forget(root: &Path, id: &str) -> Result<bool, PluginError> {
    let mut found = false;

    edit(root, |rules| {
        let before = rules.len();

        rules.retain(|table| table.get("id").and_then(|value| value.as_str()) != Some(id));
        found = rules.len() < before;

        Ok(found)
    })?;

    Ok(found)
}

/// Changes the `[[rules]]` array with CHANGE, then validates and writes the file when it
/// reports a change.
fn edit(
    root: &Path,
    change: impl FnOnce(&mut toml_edit::ArrayOfTables) -> Result<bool, PluginError>,
) -> Result<(), PluginError> {
    let path = root.join("config.toml");
    let text = crate::load_config_text(root)?.ok_or("Configuration was not found.")?;
    let mut document = text.parse::<toml_edit::DocumentMut>()?;

    if !document.contains_key("rules") {
        document["rules"] = toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
    }

    let rules = document["rules"]
        .as_array_of_tables_mut()
        .ok_or("`rules` in config.toml is not a list of rules.")?;

    if !change(rules)? {
        return Ok(());
    }

    if rules.is_empty() {
        document.remove("rules");
    }

    let rendered = document.to_string();
    let config: Config = toml::from_str(&rendered)?;

    config.validate().map_err(|error| -> PluginError { error.into() })?;
    crate::secure(&path, rendered.as_bytes())?;

    Ok(())
}

#[derive(Debug, clap::Subcommand)]
pub(crate) enum Command {
    #[command(about = "List the saved rules.")]
    Rules,
    #[command(about = "Remove a saved rule by its ID.")]
    Forget { id: String },
}

/// Runs `krabb approval rules` or `krabb approval forget`.
pub(crate) fn run(command: Command, root: &Path, json: bool) -> Result<(), PluginError> {
    match command {
        Command::Rules => {
            let config = crate::load_config_at(root)?;

            if json {
                println!("{}", serde_json::to_string_pretty(&config.rules)?);
            } else if config.rules.is_empty() {
                println!("No rules are saved.\nChoose Always allow on an approval to save one.");
            } else {
                println!("{}", listing(&config.rules));
            }
        }

        Command::Forget { id } => {
            let found = forget(root, &id)?;

            match (json, found) {
                (true, _) => println!("{}", serde_json::json!({"id": id, "removed": found})),
                (false, true) => println!("Forgot rule {id}. Krabb asks again for that action."),
                (false, false) => {
                    return Err(format!(
                        "No rule has the ID {id}.\nList them with `krabb approval rules`."
                    )
                    .into());
                }
            }
        }
    }

    Ok(())
}

/// The rules as `id  scope  tool  what`, one per line.
fn listing(rules: &[Rule]) -> String {
    let lines: Vec<String> = rules
        .iter()
        .map(|rule| {
            let what = rule.command.as_deref().or(rule.path.as_deref()).unwrap_or_default();

            format!(
                "  {}  {}  {}  {what:?}, saved by {}",
                rule.id, rule.scope, rule.tool, rule.saved_by
            )
        })
        .collect();

    format!("Saved rules\n{}", lines.join("\n"))
}

#[cfg(test)]
mod tests;
