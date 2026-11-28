#![forbid(unsafe_code)]

mod server;

use krabb_core::{
    Error,
    context::{Profile, Window},
    failure::Kind,
    plugin::{Emitter, serve_events_concurrent},
    types::{Capability, CommandSpec, Hello, ModelRequest, Protocol, Request, Response},
};

use krabb_openai::chat;
use serde_json::json;
use tokio::time::Duration;

const PROVIDER: &str = "The local server";

const NO_TOOLS: &str = "The local server rejected tool calls, so Krabb cannot act through this \
     model.\nStart the server with tool calling enabled (vLLM: --enable-auto-tool-choice with a \
     --tool-call-parser; llama.cpp: --jinja) or pick a model that supports tools.";

#[tokio::main]
async fn main() -> std::process::ExitCode {
    krabb_log::run_plugin("local", plugin_main()).await
}

async fn plugin_main() -> krabb_core::Result<()> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|error| Error::Denied(format!("Local server client failed: {error}.")))?;

    let configured = std::env::var("KRABB_LOCAL_URL").unwrap_or_default();
    let server = std::sync::Arc::new(server::Server::new(&configured, server::KNOWN));

    serve_events_concurrent(
        Hello {
            formats: vec![],
            protocol: Protocol::CURRENT,
            id: "local".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            capabilities: vec![Capability::Model, Capability::Vision, Capability::Context],
            commands: vec![CommandSpec {
                name: "local".into(),
                description: "List the models the local inference server offers.".into(),
                interactive: false,
            }],
        },
        move |request, emitter| {
            let client = client.clone();
            let server = std::sync::Arc::clone(&server);

            async move { handle(&client, request, emitter, &server).await }
        },
    )
    .await
}

/// Answers one host request for SERVER.
async fn handle(
    client: &reqwest::Client,
    request: Request,
    mut emitter: Emitter,
    server: &server::Server,
) -> krabb_core::Result<Option<Response>> {
    let (id, method, params) = match request {
        Request::Call { id, method, params, .. } => (id, method, params),
        Request::Note { .. } => return Ok(None),
    };

    if method == "command" && params["args"] != json!(["models"]) {
        return Err(Error::Denied(
            "Unknown local command.\nList the models with `krabb local models`.".into(),
        ));
    }

    let credential = credential();

    if method == "check" && server.automatic() && credential.is_some() {
        return Err(Error::Denied(
            "A key is sent only to an address you set, so a found server never gets it.\nSet the \
             server address with `krabb local setup`, or clear the key."
                .into(),
        ));
    }

    let (base, key) = match method.as_str() {
        "check" | "command" | "model.info" | "generate" => {
            server.base(client, credential.as_deref()).await?
        }

        _ => return Ok(None),
    };

    let result = answer(client, id, &method, params, &base, key, &mut emitter).await;

    match &result {
        Err(Error::Failed { failure, .. })
            if matches!(failure.kind, Kind::Invalid | Kind::Cancelled) => {}
        Err(_) => server.forget(),
        Ok(_) => {}
    }

    result
}

async fn answer(
    client: &reqwest::Client,
    id: u64,
    method: &str,
    params: serde_json::Value,
    base: &str,
    key: Option<&str>,
    emitter: &mut Emitter,
) -> krabb_core::Result<Option<Response>> {
    match method {
        "check" => {
            server::models(client, base, key).await?;

            Ok(Some(Response::ok(id, json!({}))))
        }

        "command" => {
            if params["args"] != json!(["models"]) {
                return Err(Error::Denied(
                    "Unknown local command.\nList the models with `krabb local models`.".into(),
                ));
            }

            let models = server::models(client, base, key).await?;

            Ok(Some(Response::ok(id, json!({"text": server::listing(base, &models)}))))
        }

        "model.info" => {
            let models = server::models(client, base, key).await?;
            let model = server::resolve(params["model"].as_str().unwrap_or("default"), &models)?;
            let entry = models.iter().find(|entry| entry["id"] == model.as_str());

            let configured = std::env::var("KRABB_LOCAL_CONTEXT_LENGTH").unwrap_or_default();
            let profile = profile(&configured, server::context(entry))?;

            Ok(Some(Response::ok(id, serde_json::to_value(profile)?)))
        }

        "generate" => {
            let input: ModelRequest = serde_json::from_value(params)?;

            generate(client, id, input, base, key, emitter).await
        }

        _ => Ok(None),
    }
}

async fn generate(
    client: &reqwest::Client,
    id: u64,
    input: ModelRequest,
    base: &str,
    key: Option<&str>,
    emitter: &mut Emitter,
) -> krabb_core::Result<Option<Response>> {
    let model = match input.model.trim() {
        "" | "default" | "unset" => {
            server::resolve(&input.model, &server::models(client, base, key).await?)?
        }

        model => model.to_owned(),
    };

    let mut body = chat::body(PROVIDER, &input, &model, input.stream)?;

    // Older local servers read only `max_tokens`, so both carry the same budget.
    if let Some(budget) = body.get("max_completion_tokens").cloned() {
        body["max_tokens"] = budget;
    }

    let tools = body["tools"].as_array().is_some_and(|tools| !tools.is_empty());
    let api = chat::Api { client, key, base, provider: PROVIDER };
    let emitter = input.stream.then_some(emitter);

    api.generate(id, &body, emitter).await.map_err(|error| refused(error, tools))
}

/// Phrases servers use when tool calling is off, as opposed to other tool errors.
const TOOLS_OFF: &[&str] = &[
    "enable-auto-tool-choice",
    "--jinja",
    "does not support tools",
    "tool_choice",
    "tools are not supported",
];

/// Names a server's refusal of tool calls plainly, since Krabb never emulates them, and keeps
/// the server's own reason.
fn refused(error: Error, tools: bool) -> Error {
    match error {
        Error::Failed { message, failure }
            if tools && failure.kind == Kind::Invalid && tools_off(&message) =>
        {
            failure.error(format!("{NO_TOOLS}\n{message}"))
        }

        error => error,
    }
}

/// The context window: the configured token count, or the one the server reported.
fn profile(configured: &str, reported: u64) -> krabb_core::Result<Profile> {
    let tokens = match configured.trim() {
        "" | "auto" => reported,

        value => value.parse::<u32>().ok().filter(|value| *value > 1).map(u64::from).ok_or_else(
            || {
                Error::Denied(
                    "Context length must be auto or a positive token count greater than one."
                        .into(),
                )
            },
        )?,
    };

    let window = Window { tokens, output: 4096.min(tokens / 2) };

    window.input()?;

    Ok(Profile { execution: None, window: Some(window), compact: false, search: false })
}

fn tools_off(message: &str) -> bool {
    let message = message.to_ascii_lowercase();

    TOOLS_OFF.iter().any(|phrase| message.contains(phrase))
}

fn credential() -> Option<String> {
    std::env::var("KRABB_LOCAL_KEY").ok().filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests;
