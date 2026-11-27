//! The model's one-line purpose for a tool call that may need approval.
//!
//! The host offers an optional `purpose` property on those tools and removes it before the
//! call is validated, bound to an approval, or dispatched, so plugins and approval identity
//! never see it.

use crate::*;

pub(crate) const NAME: &str = "purpose";
pub(crate) const LIMIT: usize = 160;

const DESCRIPTION: &str = "One short sentence, in the user's language, saying why you need this \
    action. The approval prompt shows it as your description.";

/// Adds the optional purpose property to each tool in SPECS that may need approval.
pub(crate) fn offer(specs: &mut [ToolSpec]) {
    for spec in specs.iter_mut().filter(|spec| offered(spec)) {
        if let Some(properties) = spec.schema["properties"].as_object_mut() {
            properties.insert(
                NAME.into(),
                serde_json::json!({"type": "string", "description": DESCRIPTION}),
            );
        }
    }
}

/// Removes the purpose the host offered from ARGS, returning it bounded to one line.
///
/// A tool whose own schema in SPECS declares `purpose` keeps it, because the host never
/// offered one there.
pub(crate) fn take(specs: &[ToolSpec], name: &str, args: &mut Value) -> Option<String> {
    if !approvable(name) || specs.iter().any(|spec| spec.name == name && !offered(spec)) {
        return None;
    }

    let purpose = args.as_object_mut()?.remove(NAME)?;

    clean(purpose.as_str()?)
}

/// The labelled line PURPOSE takes in an approval prompt.
pub(crate) fn line(purpose: &str) -> String {
    format!("Krabb says: {purpose}")
}

fn clean(purpose: &str) -> Option<String> {
    let purpose = purpose.trim();

    (!purpose.is_empty()).then(|| super::facts::clipped(purpose, LIMIT))
}

fn offered(spec: &ToolSpec) -> bool {
    approvable(&spec.name) && spec.schema["properties"].get(NAME).is_none()
}

fn approvable(name: &str) -> bool {
    mutating(name, &Value::Null) || name == routing::NAME
}

#[cfg(test)]
mod tests;
