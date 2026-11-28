//! `krabb plugin check <id> --for <capability>`: read-only conformance probes for the protocol
//! methods the host calls on a primary capability.

use std::{path::Path, time::Duration};

use krabb_core::{
    Capability, Composition,
    types::{Request, Response},
};

use serde_json::{Value, json};

use crate::PluginError;

const LIMIT: Duration = Duration::from_secs(20);

/// What a probe's answer must show.
#[derive(Clone, Copy, Debug)]
enum Expect {
    /// A result with an `items` array.
    Items,
    /// A result object.
    Object,
    /// Any answer other than an unknown method, since the probe sends no usable input.
    Known,
}

struct Probe {
    method: &'static str,
    params: Value,
    expect: Expect,
}

/// Probes never change plugin data: reads only, and calls without the input a write needs.
fn probes(capability: &Capability) -> Vec<Probe> {
    let probe = |method, params, expect| Probe { method, params, expect };

    match capability {
        Capability::Memory => vec![
            probe("index", json!({"scope": "global"}), Expect::Items),
            probe(
                "search",
                json!({"query": "krabb conformance", "scope": "global"}),
                Expect::Items,
            ),
            probe("list", json!({"scope": "global"}), Expect::Items),
        ],

        Capability::Scheduler => vec![
            probe("list", json!({"page": 1}), Expect::Items),
            probe("due", json!({"at": 0}), Expect::Items),
        ],

        Capability::Store => {
            vec![probe("get", json!({"key": "krabb:conformance"}), Expect::Object)]
        }

        Capability::Speech => vec![probe("transcribe", json!({}), Expect::Known)],

        Capability::Runner => {
            vec![probe(krabb_core::operation::LIST, json!({}), Expect::Object)]
        }

        _ => Vec::new(),
    }
}

fn verdict(expect: Expect, response: &Response) -> Result<(), String> {
    if let Some(error) = &response.error {
        return match expect {
            Expect::Known if error.code != -32601 => Ok(()),
            _ if error.code == -32601 => Err("the method is not implemented".into()),
            _ => Err(crate::sentence(crate::clip(error.message.clone(), 200))),
        };
    }

    let result = response.result.as_ref();

    match expect {
        Expect::Items if result.is_some_and(|value| value["items"].is_array()) => Ok(()),
        Expect::Items => Err("the result has no items list".into()),
        Expect::Object if result.is_some_and(Value::is_object) => Ok(()),
        Expect::Object => Err("the result is not an object".into()),
        Expect::Known => Ok(()),
    }
}

pub(crate) async fn command(
    root: &Path,
    id: &str,
    name: &str,
    json: bool,
) -> Result<(), PluginError> {
    let capability =
        Capability::parse(name).ok_or_else(|| format!("Unknown capability {name}."))?;

    if capability.composition() != Composition::Primary {
        return Err(format!(
            "{name} has no conformance suite, because {name} plugins {}.",
            super::describe(capability.composition())
        )
        .into());
    }

    let lock = crate::load_lock_at(root)?;

    if !lock.plugins.get(id).is_some_and(|entry| entry.capabilities.iter().any(|c| c == name)) {
        return Err(format!(
            "Plugin {id} is not installed as a {name} plugin.\nList installed plugins with \
             `krabb plugin list`."
        )
        .into());
    }

    let binary =
        crate::binary_at(id, root).ok_or_else(|| format!("Plugin binary was not found: {id}."))?;
    let config = crate::load_config_at(root)?;
    let mut process =
        crate::launch(root, binary, id, Some(capability.clone()), false, &config).await?;

    let mut rows = Vec::new();

    for (index, probe) in probes(&capability).into_iter().enumerate() {
        let request = Request::call(index as u64 + 2, probe.method, probe.params);
        let outcome = match tokio::time::timeout(LIMIT, process.call(request)).await {
            Ok(Ok(response)) => verdict(probe.expect, &response),
            Ok(Err(error)) => Err(crate::sentence(error.to_string())),
            Err(_) => Err("the call timed out".into()),
        };

        rows.push((probe.method, outcome));
    }

    let _ = process.stop().await;
    let failed = rows.iter().filter(|(_, outcome)| outcome.is_err()).count();

    if json {
        let checks = rows
            .iter()
            .map(|(method, outcome)| match outcome {
                Ok(()) => json!({"method": method, "status": "passed"}),
                Err(detail) => json!({"method": method, "status": "failed", "detail": detail}),
            })
            .collect::<Vec<_>>();

        let value = json!({
            "id": id, "capability": name, "status": if failed == 0 { "passed" } else { "failed" },
            "checks": checks,
        });

        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("Plugin {id} as {name}");

        for (method, outcome) in &rows {
            match outcome {
                Ok(()) => println!("  ✓ {method}"),
                Err(detail) => println!("  ✗ {method}: {detail}"),
            }
        }
    }

    if failed > 0 {
        return Err(format!(
            "{failed} of {} {name} checks failed for {id}.\nKeep the current {name} plugin until \
             {id} passes.",
            rows.len()
        )
        .into());
    }

    if !json {
        println!("\nSelect it with `krabb plugin use {id} --for {name}`.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(result: Option<Value>, error: Option<(i64, &str)>) -> Response {
        serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": 2, "result": result,
            "error": error.map(|(code, message)| json!({"code": code, "message": message})),
        }))
        .unwrap()
    }

    #[test]
    fn every_primary_capability_has_probes_and_others_have_none() {
        for capability in Capability::ALL {
            let primary = capability.composition() == Composition::Primary;

            assert_eq!(!probes(&capability).is_empty(), primary, "{}", capability.name());
        }
    }

    #[test]
    fn verdicts_require_the_expected_shape() {
        let items = response(Some(json!({"items": []})), None);
        let missing = response(None, Some((-32601, "Method not found")));
        let refused = response(None, Some((-32602, "path is required")));

        assert!(verdict(Expect::Items, &items).is_ok());
        assert!(verdict(Expect::Items, &response(Some(json!({})), None)).is_err());
        assert!(verdict(Expect::Object, &items).is_ok());
        assert!(verdict(Expect::Object, &response(Some(json!([])), None)).is_err());
        assert!(verdict(Expect::Known, &refused).is_ok());
        assert_eq!(verdict(Expect::Known, &missing).unwrap_err(), "the method is not implemented");
        assert!(verdict(Expect::Items, &refused).unwrap_err().contains("path is required"));
    }
}
