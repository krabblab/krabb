//! The state contract a plugin version declares: the schema its saved state uses and the older
//! schemas it migrates itself when it starts. The host never migrates plugin state; it previews
//! each change and refuses one the new version cannot read before anything is replaced.

use serde::{Deserialize, Serialize};

/// How many older schemas one version may migrate from, keeping manifests small.
const ORIGINS: usize = 64;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    /// The schema of the state this version reads and writes, starting at 1.
    pub schema: u32,
    /// Older schemas this version migrates to `schema` when it starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from: Vec<u32>,
}

impl State {
    pub(crate) fn validate(&self) -> Result<(), String> {
        let mut origins = self.from.clone();

        origins.sort_unstable();
        origins.dedup();

        if self.schema == 0
            || origins.len() != self.from.len()
            || self.from.len() > ORIGINS
            || self.from.iter().any(|origin| *origin == 0 || *origin >= self.schema)
        {
            return Err(format!(
                "Plugin state must name a schema from 1 and up to {ORIGINS} distinct older \
                 schemas it migrates from."
            ));
        }

        Ok(())
    }
}

/// A change of saved state that replacing a plugin brings, which the plugin applies itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Migration {
    /// The schema recorded for the installed version, or `None` when it declared none.
    pub from: Option<u32>,
    pub to: u32,
}

impl Migration {
    /// How a preview names this change.
    pub(crate) fn describe(&self) -> String {
        match self.from {
            Some(from) => format!("state: schema {from} → {} when it starts", self.to),
            None => format!("state: declares schema {}", self.to),
        }
    }
}

/// What replacing the installed version of ID with VERSION does to its saved state: nothing,
/// a migration the new version declares, or an error when it cannot read that state.
pub(crate) fn transition(
    id: &str,
    version: &str,
    installed: Option<&State>,
    next: Option<&State>,
) -> Result<Option<Migration>, String> {
    let refused = |reason: String| {
        Err(format!("Plugin {id} {version} {reason}, so it was not installed and nothing changed."))
    };

    match (installed, next) {
        (None, None) => Ok(None),
        (None, Some(next)) => Ok(Some(Migration { from: None, to: next.schema })),

        (Some(installed), None) => refused(format!(
            "declares no state schema, but the installed state uses schema {}",
            installed.schema
        )),

        (Some(installed), Some(next)) if installed.schema == next.schema => Ok(None),

        (Some(installed), Some(next)) if installed.schema > next.schema => refused(format!(
            "reads state schema {}, older than the installed schema {}",
            next.schema, installed.schema
        )),

        (Some(installed), Some(next)) if next.from.contains(&installed.schema) => {
            Ok(Some(Migration { from: Some(installed.schema), to: next.schema }))
        }

        (Some(installed), Some(next)) => refused(format!(
            "cannot migrate state from schema {} to {}",
            installed.schema, next.schema
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(schema: u32, from: &[u32]) -> State {
        State { schema, from: from.to_vec() }
    }

    #[test]
    fn manifests_name_a_schema_and_only_older_distinct_origins() {
        assert!(state(1, &[]).validate().is_ok());
        assert!(state(3, &[1, 2]).validate().is_ok());

        for invalid in [state(0, &[]), state(2, &[2]), state(3, &[1, 1]), state(2, &[0])] {
            assert!(invalid.validate().is_err(), "{invalid:?}");
        }

        let many = (1..=65).collect::<Vec<_>>();

        assert!(state(66, &many).validate().is_err());

        let parsed: State = toml::from_str("schema = 2\nfrom = [1]").unwrap();

        assert_eq!(parsed, state(2, &[1]));
        assert!(toml::from_str::<State>("schema = 2\nmigrate = true").is_err());
    }

    #[test]
    fn replacements_migrate_declared_origins_and_refuse_the_rest() {
        let check = |installed: Option<State>, next: Option<State>| {
            transition("memory", "0.2.0", installed.as_ref(), next.as_ref())
        };

        assert_eq!(check(None, None), Ok(None));
        assert_eq!(check(Some(state(2, &[1])), Some(state(2, &[1]))), Ok(None));
        assert_eq!(check(None, Some(state(1, &[]))), Ok(Some(Migration { from: None, to: 1 })));

        assert_eq!(
            check(Some(state(1, &[])), Some(state(3, &[1, 2]))),
            Ok(Some(Migration { from: Some(1), to: 3 }))
        );

        for (installed, next, reason) in [
            (state(3, &[]), Some(state(2, &[1])), "older than the installed schema 3"),
            (state(1, &[]), Some(state(3, &[2])), "cannot migrate state from schema 1 to 3"),
            (state(1, &[]), None, "declares no state schema"),
        ] {
            let error = check(Some(installed), next).unwrap_err();

            assert!(error.contains(reason), "{error}");
            assert!(error.starts_with("Plugin memory 0.2.0 "), "{error}");
            assert!(error.ends_with("nothing changed."), "{error}");
        }

        assert_eq!(
            Migration { from: Some(1), to: 2 }.describe(),
            "state: schema 1 → 2 when it starts"
        );

        assert_eq!(Migration { from: None, to: 1 }.describe(), "state: declares schema 1");
    }

    #[cfg(unix)]
    fn source(root: &std::path::Path, version: &str, state: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let source = root.join(format!("source-{version}"));
        let binary = source.join("bin").join(crate::plugin_name("keeper"));

        std::fs::create_dir_all(source.join("bin")).unwrap();

        std::fs::write(
            source.join("krabb-plugin.toml"),
            format!(
                "id='keeper'\nversion='{version}'\nprotocol={{major=0,minor=1}}\n\
                 capabilities=['memory']\n{state}"
            ),
        )
        .unwrap();

        let hello = format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"protocol":{{"major":0,"minor":1}},"id":"keeper","version":"{version}","capabilities":["memory"]}}}}"#
        );

        std::fs::write(
            &binary,
            format!(
                "#!/bin/sh\nwhile IFS= read -r line; do\ncase \"$line\" in\n*hello*) printf '%s\\n' \
                 '{hello}' ;;\n*shutdown*) printf '%s\\n' \
                 '{{\"jsonrpc\":\"2.0\",\"id\":9999,\"result\":{{\"ok\":true}}}}'; exit 0 ;;\nesac\n\
                 done\n"
            ),
        )
        .unwrap();

        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();

        source
    }

    #[cfg(unix)]
    fn install(root: &std::path::Path, source: &std::path::Path) -> Result<(), String> {
        crate::link_at(
            crate::Source {
                id: "keeper".into(),
                source: Some(source.display().to_string()),
                revision: None,
                version: None,
                yes: true,
            },
            false,
            root,
        )
        .map(drop)
        .map_err(|error| error.to_string())
    }

    #[cfg(unix)]
    #[test]
    fn installs_record_the_state_schema_and_refuse_unreadable_replacements() {
        let root = crate::tests::test_root("plugin-state");

        crate::init_at(&root).unwrap();

        let recorded = |root: &std::path::Path| {
            crate::load_lock_at(root).unwrap().plugins["keeper"].state.clone()
        };

        install(&root, &source(&root, "0.1.0", "[state]\nschema = 2\nfrom = [1]\n")).unwrap();

        assert_eq!(recorded(&root), Some(state(2, &[1])));

        for (version, declared, reason) in [
            ("0.0.9", "[state]\nschema = 1\n", "older than the installed schema 2"),
            ("0.2.0", "[state]\nschema = 3\n", "cannot migrate state from schema 2 to 3"),
            ("0.2.1", "", "declares no state schema"),
        ] {
            let error = install(&root, &source(&root, version, declared)).unwrap_err();

            assert!(error.contains(reason), "{error}");
            assert_eq!(crate::load_lock_at(&root).unwrap().plugins["keeper"].version, "0.1.0");
        }

        install(&root, &source(&root, "0.3.0", "[state]\nschema = 3\nfrom = [1, 2]\n")).unwrap();

        assert_eq!(recorded(&root), Some(state(3, &[1, 2])));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn older_locks_and_manifests_without_state_still_load() {
        let entry: crate::Entry = serde_json::from_value(serde_json::json!({
            "source": "index:memory",
            "version": "0.1.0",
            "protocol": {"major": 0, "minor": 1},
            "capabilities": ["memory"],
        }))
        .unwrap();

        assert_eq!(entry.state, None);
        assert!(serde_json::to_value(&entry).unwrap().get("state").is_none());

        let manifest: crate::Manifest = toml::from_str(
            "id='memory'\nversion='0.1.0'\nprotocol={major=0,minor=1}\ncapabilities=['memory']\n",
        )
        .unwrap();

        assert_eq!(manifest.state, None);
        assert!(manifest.validate().is_ok());

        let declared: crate::Manifest = toml::from_str(
            "id='memory'\nversion='0.2.0'\nprotocol={major=0,minor=1}\ncapabilities=['memory']\n\
             [state]\nschema=2\nfrom=[1]\n",
        )
        .unwrap();

        assert_eq!(declared.state, Some(state(2, &[1])));
        assert!(declared.validate().is_ok());

        let invalid: crate::Manifest = toml::from_str(
            "id='memory'\nversion='0.2.0'\nprotocol={major=0,minor=1}\ncapabilities=['memory']\n\
             [state]\nschema=1\nfrom=[1]\n",
        )
        .unwrap();

        assert!(invalid.validate().is_err());
    }
}
