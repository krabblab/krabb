//! Runs a plugin's optional connection test with its saved credentials.

use std::{path::Path, time::Duration};

use krabb_core::types::Request;
use serde_json::{Value, json};

pub(super) const METHOD: &str = "check";
const LIMIT: Duration = Duration::from_secs(20);
const DETAIL: usize = 200;
const UNAVAILABLE: &str = "The connection test could not run; the settings were saved anyway.";

/// Reports `passed`, `failed`, or `skipped`; plugin failures never fail the setup.
pub(super) async fn run(root: &Path, id: &str) -> Value {
    let supported = crate::read_manifest_checked(&root.join("plugins").join(id))
        .is_ok_and(|manifest| manifest.check);

    if !supported {
        return json!({"status": "skipped"});
    }

    match tokio::time::timeout(LIMIT, attempt(root, id)).await {
        Ok(outcome) => outcome,
        Err(_) => json!({"status": "failed", "detail": "The connection test timed out."}),
    }
}

async fn attempt(root: &Path, id: &str) -> Value {
    let Some(response) = call(root, id).await else {
        return json!({"status": "failed", "detail": UNAVAILABLE});
    };

    if let Some(error) = response.error {
        return match error.code {
            -32601 => json!({"status": "skipped"}),
            _ => json!({"status": "failed", "detail": detail(&error.message)}),
        };
    }

    json!({"status": "passed"})
}

/// Starts the installed plugin with its saved values and sends one `check` request.
async fn call(root: &Path, id: &str) -> Option<krabb_core::types::Response> {
    let binary = crate::binary_at(id, root)?;
    let config = crate::load_config_at(root).ok()?;
    let mut process = crate::launch(root, binary, id, None, false, &config).await.ok()?;
    let response = process.call(Request::call(2, METHOD, json!({}))).await;
    let _ = process.stop().await;

    response.ok()
}

/// Bounds plugin text and strips anything shaped like a credential.
fn detail(text: &str) -> String {
    crate::clip(crate::redact_diagnostic(text.trim()), DETAIL)
}

/// Renders the result as one line for terminal output.
pub(super) fn line(result: &Value) -> Option<String> {
    match result["status"].as_str()? {
        "passed" => Some("Connection test passed.".into()),

        "failed" => Some(format!(
            "Connection test failed: {}",
            crate::sentence(result["detail"].as_str().unwrap_or(UNAVAILABLE))
        )),

        _ => None,
    }
}
