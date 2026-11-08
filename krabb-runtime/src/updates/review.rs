use crate::{Entry, Manifest, diagnostic};
use serde_json::{Value, json};

pub(crate) fn declarations(previous: &Entry, next: &Manifest) -> Value {
    let mut changes = serde_json::Map::new();

    for (name, before, after) in [
        ("capabilities", json!(previous.capabilities), json!(next.capabilities)),
        ("permissions", json!(previous.permissions), json!(next.permissions)),
        ("secrets", json!(previous.variables), json!(next.variables)),
        ("settings", json!(previous.settings), json!(next.settings)),
        ("commands", json!(previous.commands), json!(next.commands)),
    ] {
        if before != after {
            changes.insert(name.into(), json!({"before": before, "after": after}));
        }
    }

    Value::Object(changes)
}

pub(crate) fn review(changes: &Value) -> Vec<String> {
    changes
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, change)| {
            format!(
                "{name}: {} → {}",
                diagnostic(change["before"].to_string()),
                diagnostic(change["after"].to_string())
            )
        })
        .collect()
}
