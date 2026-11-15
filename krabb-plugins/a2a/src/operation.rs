use super::*;

use krabb_core::{
    operation::{self, Query, Receipt, Spec, Start, State},
    types::ToolResult,
};

use serde::{Deserialize, Serialize};

/// Longest remote task identifier or context identifier kept in a handle.
const ID_LIMIT: usize = 256;
const RESULT_LIMIT: usize = operation::LIMIT - 16 * 1024;
const SHORTENED: &str = "[The remote result was shortened to fit the operation receipt limit.]";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    prompt: String,
    card: Option<String>,
    context: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Handle {
    card: String,
    endpoint: String,
    task: String,
}

fn spec(config: &A2aConfig) -> Spec {
    let configured = config.card.as_deref().is_some_and(|card| validate_url(card).is_ok());
    let required = if configured { json!(["prompt"]) } else { json!(["prompt", "card"]) };

    Spec {
        name: "delegate".into(),
        description: "Start a remote agent task and inspect its durable handle later.".into(),
        input: json!({"type":"object", "properties":{
            "prompt":{"type":"string"}, "card":{"type":"string"}, "context":{"type":"string"}
        }, "required":required, "additionalProperties":false}),
        cancel: true,
    }
}

pub(super) async fn call(
    client: &reqwest::Client,
    id: u64,
    method: &str,
    params: Value,
    emitter: &mut Emitter,
    config: &A2aConfig,
) -> krabb_core::Result<Response> {
    if method == operation::LIST {
        return Ok(Response::ok(id, json!({"operations":[spec(config)]})));
    }

    let receipt = match method {
        operation::START => start(client, serde_json::from_value(params)?, emitter, config).await?,

        operation::INSPECT | operation::CANCEL | operation::RESULT => {
            inspect(client, method, serde_json::from_value(params)?, emitter, config).await?
        }

        _ => return Ok(Response::fail(id, -32601, "Unsupported operation method.")),
    };

    receipt.validate()?;
    Ok(Response::ok(id, serde_json::to_value(receipt)?))
}

/// Checks the handle against the longest task identifier: a control character is the longest
/// JSON escape, so whatever identifier the backend returns still fits once the task started.
fn handle_fits(card: &str, endpoint: &str) -> krabb_core::Result<()> {
    let size = serde_json::to_string(&Handle {
        card: card.into(),
        endpoint: endpoint.into(),
        task: "\u{1}".repeat(ID_LIMIT),
    })?
    .len();

    if size > operation::HANDLE_LIMIT {
        return Err(krabb_core::Error::Denied(
            "The operation endpoint metadata exceeds the handle limit.".into(),
        ));
    }

    Ok(())
}

async fn start(
    client: &reqwest::Client,
    start: Start,
    emitter: &mut Emitter,
    config: &A2aConfig,
) -> krabb_core::Result<Receipt> {
    start.validate()?;
    check_name(&start.name)?;

    let args: Arguments = serde_json::from_value(start.arguments)?;

    if args.prompt.trim().is_empty()
        || args.context.as_ref().is_some_and(|context| context.len() > ID_LIMIT)
    {
        return Err(krabb_core::Error::Denied("A bounded prompt and context are required.".into()));
    }

    let card = args.card.or_else(|| config.card.clone()).ok_or_else(|| {
        krabb_core::Error::Denied("An agent card is required for this operation.".into())
    })?;

    let card = validate_url(&card)?;
    let description = discover(client, &card).await?;
    let endpoint = rpc_endpoint(&description)?;

    authorize_endpoint(&card, &endpoint, config)?;

    handle_fits(card.as_str(), endpoint.as_str())?;

    let result = dispatch(
        client,
        A2aRequest {
            action: "send".into(),
            card: Some(card.to_string()),
            context: args.context,
            task: None,
            prompt: args.prompt,
            stream: Some(false),
            return_immediately: true,
            message: Some(start.id),
            endpoint: Some(endpoint.to_string()),
        },
        &card,
        &description,
        emitter,
        config,
    )
    .await?;

    if result["taskId"].as_str().is_some_and(|task| task.is_empty() || task.len() > ID_LIMIT) {
        return Err(krabb_core::Error::Protocol(
            concat!(
                "The backend returned an unusable task identifier. ",
                "Review its effects before another start."
            )
            .into(),
        ));
    }

    let handle = result["taskId"].as_str().map(|task| Handle {
        card: card.to_string(),
        endpoint: endpoint.to_string(),
        task: task.into(),
    });

    receipt(result, handle.map(|handle| serde_json::to_string(&handle)).transpose()?)
}

async fn inspect(
    client: &reqwest::Client,
    method: &str,
    query: Query,
    emitter: &mut Emitter,
    config: &A2aConfig,
) -> krabb_core::Result<Receipt> {
    query.validate()?;
    check_name(&query.name)?;

    let handle: Handle = serde_json::from_str(&query.handle)?;

    validate_url(&handle.card)?;
    validate_url(&handle.endpoint)?;

    if handle.task.is_empty() || handle.task.len() > ID_LIMIT {
        return Err(krabb_core::Error::Denied("Invalid remote operation handle.".into()));
    }

    let result = execute_with(
        client,
        A2aRequest {
            action: if method == operation::CANCEL { "cancel" } else { "status" }.into(),
            card: Some(handle.card),
            context: None,
            task: Some(handle.task),
            prompt: String::new(),
            stream: Some(false),
            return_immediately: true,
            message: None,
            endpoint: Some(handle.endpoint),
        },
        emitter,
        config,
    )
    .await?;

    let receipt = receipt(result, Some(query.handle))?;

    if method == operation::RESULT && !receipt.state.terminal() {
        return Err(krabb_core::Error::Denied(
            "The remote operation has no final result yet.".into(),
        ));
    }

    Ok(receipt)
}

fn check_name(name: &str) -> krabb_core::Result<()> {
    if name != "delegate" {
        return Err(krabb_core::Error::Denied("Unknown A2A operation.".into()));
    }

    Ok(())
}

fn receipt(value: Value, handle: Option<String>) -> krabb_core::Result<Receipt> {
    let state = match value["status"].as_str().unwrap_or_default() {
        "TASK_STATE_SUBMITTED" | "submitted" => State::Pending,
        "TASK_STATE_WORKING" | "working" => State::Running,
        "TASK_STATE_INPUT_REQUIRED"
        | "TASK_STATE_AUTH_REQUIRED"
        | "input-required"
        | "auth-required" => State::Waiting,
        "TASK_STATE_COMPLETED" | "completed" | "message" => State::Completed,
        "TASK_STATE_FAILED" | "TASK_STATE_REJECTED" | "failed" | "rejected" => State::Failed,
        "TASK_STATE_CANCELED" | "canceled" | "cancelled" => State::Cancelled,
        _ => State::Unknown,
    };

    let detail = match state {
        State::Unknown => Some("The backend did not report a recognized operation state.".into()),

        State::Waiting => {
            let text = value["text"]
                .as_str()
                .filter(|text| !text.trim().is_empty())
                .unwrap_or("The remote agent requires input or authorization.");
            Some(clip(text, operation::DETAIL_LIMIT).into())
        }

        _ => None,
    };

    let text = value["text"].as_str().unwrap_or_default();
    let mut limit = RESULT_LIMIT;

    loop {
        let result = state.terminal().then(|| {
            let kept = clip(text, limit);

            if kept.len() == text.len() {
                ToolResult::text(text)
            } else {
                ToolResult::text(format!("{kept}\n{SHORTENED}"))
            }
        });

        let receipt = Receipt { state, handle: handle.clone(), detail: detail.clone(), result };

        match receipt.validate() {
            Err(_) if receipt.result.is_some() && limit > 0 => limit /= 2,
            checked => return checked.map(|()| receipt),
        }
    }
}

/// Keeps at most `limit` bytes of `text`, ending on a character boundary.
fn clip(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);

    while !text.is_char_boundary(end) {
        end -= 1;
    }

    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_cancelled_and_unknown_states_never_become_successful_completion() {
        for (remote, expected) in [
            ("TASK_STATE_INPUT_REQUIRED", State::Waiting),
            ("TASK_STATE_AUTH_REQUIRED", State::Waiting),
            ("TASK_STATE_FAILED", State::Failed),
            ("TASK_STATE_CANCELED", State::Cancelled),
            ("TASK_STATE_SUBMITTED", State::Pending),
            ("future-state", State::Unknown),
        ] {
            let receipt =
                receipt(json!({"status":remote, "text":"Backend status"}), Some("handle".into()))
                    .unwrap();
            assert_eq!(receipt.state, expected);
            assert_ne!(receipt.state, State::Completed);
            assert_eq!(receipt.result.is_some(), expected.terminal());
        }

        assert!(receipt(json!({"status":"TASK_STATE_WORKING"}), None).is_err());

        let reply =
            receipt(json!({"status":"message", "text":"An immediate answer"}), None).unwrap();

        assert_eq!(reply.state, State::Completed);
        assert_eq!(reply.result.unwrap(), ToolResult::text("An immediate answer"));

        let waiting = receipt(
            json!({"status":"TASK_STATE_INPUT_REQUIRED", "text":"Which dataset should I use?"}),
            Some("handle".into()),
        )
        .unwrap();

        assert_eq!(waiting.detail.as_deref(), Some("Which dataset should I use?"));
    }

    #[test]
    fn the_handle_check_reserves_room_for_an_escaped_task_identifier() {
        let card = "https://agent.example/card";

        handle_fits(card, "https://agent.example/rpc").unwrap();

        let endpoint = format!("https://agent.example/{}", "x".repeat(3_000));

        assert!(endpoint.len() + ID_LIMIT < operation::HANDLE_LIMIT - card.len() - 64);
        assert!(handle_fits(card, &endpoint).is_err());
    }

    #[test]
    fn a_large_final_result_is_shortened_to_fit_the_receipt_limit() {
        let text = "\u{e9}\"".repeat(512 * 1024);

        let receipt =
            receipt(json!({"status":"TASK_STATE_COMPLETED", "text":text}), Some("handle".into()))
                .unwrap();

        let ToolResult { content, .. } = receipt.result.unwrap();

        let krabb_core::types::ToolContent::Text { text: kept } = &content[0] else {
            panic!("The result must stay text.");
        };

        assert!(kept.ends_with(SHORTENED));
        assert!(kept.len() < text.len());
    }
}
