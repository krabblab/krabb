//! Chooses the plugins that serve each primary capability from `[plugins.<capability>]`.

mod check;
mod select;

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use krabb_core::{Capability, Composition};
use serde::{Deserialize, Serialize};

use crate::{Live, Lock, Plugins, valid};

pub(crate) use check::command as check_command;
pub(crate) use select::{forget, hold, run as use_command};

/// Additional plugins per capability, which bounds merged reads and scheduler loops.
const ALSO_LIMIT: usize = 8;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Choice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) primary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) also: Vec<String>,
}

pub(crate) type Table = BTreeMap<String, Choice>;

/// The plugins serving one primary capability: the primary takes writes and commands.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Members<T> {
    pub(crate) primary: Option<T>,
    pub(crate) also: Vec<T>,
}

impl<T> Members<T> {
    pub(crate) fn all(&self) -> impl Iterator<Item = &T> {
        self.primary.iter().chain(&self.also)
    }
}

pub(crate) fn describe(composition: Composition) -> &'static str {
    match composition {
        Composition::Stack => "all combine",
        Composition::Select => "follow the model selection",
        Composition::Primary => "have one primary",
    }
}

pub(crate) fn validate(table: &Table) -> Result<(), String> {
    for (name, choice) in table {
        let capability = Capability::parse(name)
            .ok_or_else(|| format!("plugins.{name} is not a Krabb capability."))?;
        let composition = capability.composition();

        if composition != Composition::Primary {
            return Err(format!(
                "plugins.{name} cannot be configured, because {name} plugins {}.",
                describe(composition)
            ));
        }

        if let Some(id) = choice.primary.iter().chain(&choice.also).find(|id| !valid(id)) {
            return Err(format!("plugins.{name} names an invalid plugin ID: {id}."));
        }

        if !choice.also.is_empty() && !capability.additional() {
            return Err(format!(
                "plugins.{name}.also is unavailable, because only memory and scheduler plugins \
                 combine."
            ));
        }

        if choice.also.len() > ALSO_LIMIT {
            return Err(format!("plugins.{name}.also lists more than {ALSO_LIMIT} plugins."));
        }

        let mut seen = BTreeSet::new();

        if let Some(id) = choice.primary.iter().chain(&choice.also).find(|id| !seen.insert(*id)) {
            return Err(format!("plugins.{name} lists {id} more than once."));
        }
    }

    Ok(())
}

/// Every plugin the table names must be installed and declare the capability it serves.
pub(crate) fn validate_installed(table: &Table, lock: &Lock) -> Result<(), String> {
    for (name, choice) in table {
        for id in choice.primary.iter().chain(&choice.also) {
            let Some(entry) = lock.plugins.get(id) else {
                return Err(format!("plugins.{name} names {id}, which is not installed."));
            };

            if !entry.capabilities.iter().any(|capability| capability == name) {
                return Err(format!("plugins.{name} names {id}, which does not provide {name}."));
            }
        }
    }

    Ok(())
}

/// Resolves the members among `candidates`, the plugins declaring the capability in ID order.
/// Without a configured primary the first candidate serves, as before composition existed; a
/// configured primary that is unavailable leaves the capability without one, never another.
pub(crate) fn resolve<'a>(
    capability: &Capability,
    table: &Table,
    candidates: &[&'a str],
) -> Members<&'a str> {
    if capability.composition() != Composition::Primary {
        return Members { primary: candidates.first().copied(), also: Vec::new() };
    }

    let choice = table.get(capability.name());

    let primary = match choice.and_then(|choice| choice.primary.as_deref()) {
        Some(id) => candidates.iter().copied().find(|candidate| *candidate == id),
        None => candidates.first().copied(),
    };

    let also = choice
        .map(|choice| choice.also.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|id| candidates.iter().copied().find(|candidate| candidate == id))
        .filter(|id| Some(*id) != primary)
        .collect();

    Members { primary, also }
}

/// Members from the installed plugins, for commands that run without the daemon.
pub(crate) fn installed<'a>(
    capability: &Capability,
    table: &Table,
    lock: &'a Lock,
) -> Members<&'a str> {
    let candidates = lock
        .plugins
        .iter()
        .filter(|(_, entry)| entry.capabilities.iter().any(|name| name == capability.name()))
        .map(|(id, _)| id.as_str())
        .collect::<Vec<_>>();

    resolve(capability, table, &candidates)
}

/// Each capability with an installed plugin: its composition, primary, and other plugins.
pub(crate) fn summary(table: &Table, lock: &Lock) -> Vec<serde_json::Value> {
    Capability::ALL
        .iter()
        .filter_map(|capability| {
            let plugins = lock
                .plugins
                .iter()
                .filter(|(_, entry)| {
                    entry.capabilities.iter().any(|name| name == capability.name())
                })
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>();

            if plugins.is_empty() {
                return None;
            }

            let composition = capability.composition();
            let members = installed(capability, table, lock);
            let primary = (composition == Composition::Primary).then_some(members.primary);

            Some(serde_json::json!({
                "capability": capability.name(),
                "composition": composition.name(),
                "primary": primary.flatten(),
                "also": members.also,
                "plugins": plugins,
            }))
        })
        .collect()
}

/// One summary row as text, such as `memory (primary): zep, also memory`.
pub(crate) fn line(row: &serde_json::Value) -> String {
    let names = |key: &str| {
        row[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    };

    let capability = row["capability"].as_str().unwrap_or_default();
    let composition = row["composition"].as_str().unwrap_or_default();

    if composition != Composition::Primary.name() {
        return format!("{capability} ({composition}): {}", names("plugins"));
    }

    let primary = row["primary"].as_str().unwrap_or("none selected");
    let also = names("also");

    if also.is_empty() {
        format!("{capability} (primary): {primary}")
    } else {
        format!("{capability} (primary): {primary}, also {also}")
    }
}

/// Takes `--plugin <id>` out of command arguments.
pub(crate) fn target(args: &[String]) -> Result<(Vec<String>, Option<String>), String> {
    let mut rest = Vec::with_capacity(args.len());
    let mut plugin = None;
    let mut values = args.iter();

    while let Some(argument) = values.next() {
        if argument == "--" {
            rest.push(argument.clone());
            rest.extend(values.by_ref().cloned());
            break;
        }

        let value = match argument.strip_prefix("--plugin=") {
            Some(value) => value.to_owned(),

            None if argument == "--plugin" => {
                values.next().cloned().ok_or("--plugin needs a plugin ID.")?
            }

            None => {
                rest.push(argument.clone());
                continue;
            }
        };

        if plugin.replace(value).is_some() {
            return Err("Choose one --plugin.".into());
        }
    }

    if let Some(id) = &plugin
        && !valid(id)
    {
        return Err(format!("Invalid plugin ID: {id}."));
    }

    Ok((rest, plugin))
}

impl Plugins {
    pub(crate) fn table(&self) -> Table {
        self.composition
            .lock()
            .ok()
            .and_then(|table| table.clone())
            .unwrap_or_else(|| self.config.plugins.clone())
    }

    /// A view whose scheduler is always `owner`, for the loop that runs its schedules.
    pub(crate) fn owned_by(&self, owner: &str) -> Self {
        Self { owner: Some(Arc::from(owner)), ..self.clone() }
    }

    pub(crate) async fn members(&self, capability: &Capability) -> Members<(String, Arc<Live>)> {
        let items = self.items.read().await;
        let candidates = items
            .iter()
            .filter(|(_, plugin)| plugin.supports(capability.clone()))
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>();

        let members = resolve(capability, &self.table(), &candidates);
        let entry = |id: &str| (id.to_owned(), Arc::clone(&items[id]));

        Members {
            primary: members.primary.map(entry),
            also: members.also.into_iter().map(entry).collect(),
        }
    }

    /// Whether `id` serves the capability now, as its primary or an additional plugin.
    pub(crate) async fn serves(&self, capability: &Capability, id: &str) -> bool {
        self.members(capability).await.all().any(|(member, _)| member == id)
    }

    /// The plugin a command names with `--plugin`, which must be loaded and declare it.
    pub(crate) async fn named(
        &self,
        capability: &Capability,
        id: &str,
    ) -> Result<Arc<Live>, String> {
        self.get(id)
            .await
            .filter(|plugin| plugin.supports(capability.clone()))
            .ok_or_else(|| format!("Plugin {id} is not running as a {} plugin.", capability.name()))
    }

    pub(crate) async fn primary(&self, capability: Capability) -> Option<(String, Arc<Live>)> {
        if capability == Capability::Scheduler
            && let Some(owner) = &self.owner
        {
            return self
                .get(owner)
                .await
                .filter(|plugin| plugin.supports(Capability::Scheduler))
                .map(|plugin| (owner.to_string(), plugin));
        }

        self.members(&capability).await.primary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> Table {
        #[derive(Deserialize)]
        struct File {
            plugins: Table,
        }

        toml::from_str::<File>(text).unwrap().plugins
    }

    #[test]
    fn without_configuration_the_first_plugin_serves_as_before() {
        let members = resolve(&Capability::Memory, &Table::new(), &["memory", "zep"]);

        assert_eq!(members, Members { primary: Some("memory"), also: vec![] });

        let members = resolve(&Capability::Memory, &Table::new(), &["memory"]);

        assert_eq!(members.primary, Some("memory"));
        assert_eq!(resolve(&Capability::Memory, &Table::new(), &[]).primary, None);
    }

    #[test]
    fn the_configured_primary_and_additional_plugins_serve() {
        let table = table("[plugins.memory]\nprimary = \"zep\"\nalso = [\"memory\", \"gone\"]\n");
        let members = resolve(&Capability::Memory, &table, &["memory", "zep"]);

        assert_eq!(members, Members { primary: Some("zep"), also: vec!["memory"] });
        assert_eq!(members.all().copied().collect::<Vec<_>>(), ["zep", "memory"]);
    }

    #[test]
    fn an_unavailable_primary_is_never_replaced() {
        let table = table("[plugins.memory]\nprimary = \"zep\"\nalso = [\"memory\"]\n");
        let members = resolve(&Capability::Memory, &table, &["memory"]);

        assert_eq!(members, Members { primary: None, also: vec!["memory"] });
    }

    #[test]
    fn stacked_and_selected_capabilities_ignore_the_table() {
        let table = table("[plugins.memory]\nprimary = \"zep\"\n");

        assert_eq!(
            resolve(&Capability::Tool, &table, &["browser", "tools"]).primary,
            Some("browser")
        );

        assert_eq!(resolve(&Capability::Model, &table, &["codex"]).also, Vec::<&str>::new());
    }

    #[test]
    fn validation_rejects_conflicts_and_other_compositions() {
        let cases = [
            ("[plugins.memory]\nprimary = \"zep\"\n", None),
            ("[plugins.scheduler]\nalso = [\"cron\"]\n", None),
            ("[plugins.classifier]\nprimary = \"x\"\n", Some("not a Krabb capability")),
            ("[plugins.tool]\nprimary = \"tools\"\n", Some("all combine")),
            ("[plugins.model]\nprimary = \"codex\"\n", Some("follow the model selection")),
            ("[plugins.store]\nalso = [\"other\"]\n", Some("only memory and scheduler")),
            ("[plugins.memory]\nprimary = \"zep\"\nalso = [\"zep\"]\n", Some("more than once")),
            ("[plugins.memory]\nprimary = \"Bad\"\n", Some("invalid plugin ID")),
        ];

        for (text, error) in cases {
            let result = validate(&table(text));

            match error {
                None => assert!(result.is_ok(), "{text}: {result:?}"),
                Some(error) => assert!(result.unwrap_err().contains(error), "{text}"),
            }
        }

        let many = (0..=ALSO_LIMIT).map(|index| format!("\"m{index}\"")).collect::<Vec<_>>();
        let text = format!("[plugins.memory]\nalso = [{}]\n", many.join(", "));

        assert!(validate(&table(&text)).unwrap_err().contains("more than 8"));
        assert!(toml::from_str::<Table>("memory = { primary = \"a\", first = true }").is_err());
    }

    #[test]
    fn named_plugins_must_be_installed_and_declare_the_capability() {
        let lock: Lock = serde_json::from_value(serde_json::json!({"plugins": {
            "memory": {"source": "official", "version": "1", "protocol": {"major": 0, "minor": 2},
                       "capabilities": ["memory"]},
            "sqlite": {"source": "official", "version": "1", "protocol": {"major": 0, "minor": 2},
                       "capabilities": ["store"]}
        }}))
        .unwrap();

        assert!(
            validate_installed(&table("[plugins.memory]\nprimary = \"memory\"\n"), &lock).is_ok()
        );

        let error = validate_installed(&table("[plugins.memory]\nprimary = \"zep\"\n"), &lock);

        assert!(error.unwrap_err().contains("not installed"));

        let error = validate_installed(&table("[plugins.memory]\nprimary = \"sqlite\"\n"), &lock);

        assert!(error.unwrap_err().contains("does not provide memory"));
        assert_eq!(installed(&Capability::Store, &Table::new(), &lock).primary, Some("sqlite"));
    }

    #[test]
    fn summaries_show_composition_primary_and_additional_plugins() {
        let entry = |capabilities: &[&str]| {
            serde_json::json!({"source": "official", "version": "1",
                               "protocol": {"major": 0, "minor": 2}, "capabilities": capabilities})
        };

        let lock: Lock = serde_json::from_value(serde_json::json!({"plugins": {
            "memory": entry(&["memory"]), "zep": entry(&["memory"]), "tools": entry(&["tool"]),
            "browser": entry(&["tool"]), "codex": entry(&["model", "vision", "context"]),
        }}))
        .unwrap();

        let rows =
            summary(&table("[plugins.memory]\nprimary = \"zep\"\nalso = [\"memory\"]\n"), &lock);
        let lines = rows.iter().map(line).collect::<Vec<_>>();

        assert_eq!(
            lines,
            [
                "model (select): codex",
                "context (select): codex",
                "vision (select): codex",
                "memory (primary): zep, also memory",
                "tool (stack): browser, tools",
            ]
        );

        assert_eq!(rows[3]["primary"], "zep");
        assert!(rows[4]["primary"].is_null());

        let rows = summary(&Table::new(), &lock);

        assert_eq!(line(&rows[3]), "memory (primary): memory");

        let rows = summary(&table("[plugins.memory]\nprimary = \"gone\"\n"), &lock);

        assert_eq!(line(&rows[3]), "memory (primary): none selected");
    }

    #[test]
    fn plugin_targets_are_taken_from_command_arguments() {
        let args =
            |values: &[&str]| values.iter().map(|value| (*value).to_owned()).collect::<Vec<_>>();

        assert_eq!(
            target(&args(&["search", "--plugin", "zep", "tea"])).unwrap(),
            (args(&["search", "tea"]), Some("zep".into()))
        );

        assert_eq!(
            target(&args(&["--plugin=zep", "list"])).unwrap(),
            (args(&["list"]), Some("zep".into()))
        );

        assert_eq!(
            target(&args(&["remember", "--", "--plugin", "x"])).unwrap(),
            (args(&["remember", "--", "--plugin", "x"]), None)
        );

        assert!(target(&args(&["--plugin"])).is_err());
        assert!(target(&args(&["--plugin", "a", "--plugin", "b"])).is_err());
        assert!(target(&args(&["--plugin", "Bad"])).is_err());
    }
}
