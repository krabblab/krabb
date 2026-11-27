#![forbid(unsafe_code)]

mod model;

use krabb_core::{
    plugin::{Emitter, serve_events_concurrent},
    types::{Capability, CommandSpec, Hello, ModelRequest, Protocol, Request, Response},
};

use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

mod codex;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    krabb_log::run_plugin("codex", plugin_main()).await
}

async fn plugin_main() -> krabb_core::Result<()> {
    let codex_session = Arc::new(Mutex::new(codex::session()));

    serve_events_concurrent(
        Hello {
            formats: vec![],
            protocol: Protocol::CURRENT,
            id: "codex".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            capabilities: vec![Capability::Model, Capability::Vision, Capability::Context],
            commands: vec![CommandSpec {
                name: "codex".into(),
                description: "Manage Codex sign-in and list available models.".into(),
                interactive: false,
            }],
        },
        move |request, emitter| {
            let codex_session = Arc::clone(&codex_session);
            async move { generate_with_session(request, emitter, codex_session).await }
        },
    )
    .await
}

async fn generate_with_session(
    request: Request,
    emitter: Emitter,
    codex_session: Arc<Mutex<codex::Session>>,
) -> krabb_core::Result<Option<Response>> {
    let (id, method, params) = match request {
        Request::Call { id, method, params, .. } => (id, method, params),
        Request::Note { .. } => return Ok(None),
    };

    if method == "check" {
        codex::check().await?;

        return Ok(Some(Response::ok(id, json!({}))));
    }

    if method == "command" {
        let text = codex::command(&params, emitter).await?;

        return Ok(Some(Response::ok(id, json!({"text": text}))));
    }

    if method == "model.info" {
        let model = model::resolve(params["model"].as_str().unwrap_or("default"));

        let profile = krabb_openai::profile(
            &model,
            &std::env::var("KRABB_CODEX_CONTEXT_LENGTH").unwrap_or_default(),
        )?;

        return Ok(Some(Response::ok(id, serde_json::to_value(profile)?)));
    }

    if method != "generate" {
        return Ok(None);
    }

    let input: ModelRequest = serde_json::from_value(params)?;

    codex_session.lock().await.generate(id, input, emitter).await
}

#[cfg(test)]
mod tests {
    use super::generate_with_session;

    use krabb_core::{plugin::Emitter, types::Request};
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[tokio::test]
    async fn notes_and_unknown_methods_get_no_reply() {
        let (output, _receiver) = tokio::sync::mpsc::channel(8);
        let session = || Arc::new(Mutex::new(crate::codex::session()));

        let note =
            Request::Note { jsonrpc: "2.0".into(), method: "cancel".into(), params: json!({}) };

        let unknown = Request::call(1, "unknown", json!({}));

        assert!(
            generate_with_session(note, Emitter::new(output.clone()), session())
                .await
                .unwrap()
                .is_none()
        );

        assert!(
            generate_with_session(unknown, Emitter::new(output), session())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn model_info_describes_the_resolved_model() {
        let (output, _receiver) = tokio::sync::mpsc::channel(8);
        let request = Request::call(2, "model.info", json!({"model": "default"}));
        let session = Arc::new(Mutex::new(crate::codex::session()));

        let reply = generate_with_session(request, Emitter::new(output), session).await.unwrap();

        assert!(reply.is_some());
    }
}
