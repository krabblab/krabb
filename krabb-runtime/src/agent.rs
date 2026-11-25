//! Sends one request to an agent plugin and relays its streamed reply.

use crate::*;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Bridge {
    pub(crate) approval: ApprovalMode,
    pub(crate) limit: Duration,
}

impl Bridge {
    pub(crate) fn new(config: &Config) -> Self {
        let mut limit = Duration::from_secs(config.inference.max_seconds);

        if let Some(ceiling) = config.inference.task_millis() {
            limit = limit.min(Duration::from_millis(ceiling));
        }

        Self { approval: config.approval_mode(), limit }
    }

    pub(crate) fn expired() -> krabb_core::Error {
        krabb_core::Error::Limit("The model request exceeded its host inference allowance.".into())
    }
}

pub(crate) async fn agent_request(
    request: Request,
    model: Arc<AsyncMutex<Option<Process>>>,
    tools: Arc<AsyncMutex<Option<Process>>>,
    workspace: PathBuf,
    bridge: Bridge,
    json: bool,
    events: tokio::sync::mpsc::UnboundedSender<Request>,
) -> std::result::Result<Response, krabb_core::Error> {
    let Request::Call { id, method, params, .. } = request else {
        return Err(krabb_core::Error::Protocol("Agent host requests require an ID.".into()));
    };

    match method.as_str() {
        "host/model" => {
            selected_workspace(&params, &workspace)?;
            let mut model = model.lock().await;
            let Some(provider) = model.as_mut() else {
                return Ok(Response::fail(id, -32000, "No intelligence provider is available."));
            };

            match tokio::time::timeout(
                bridge.limit,
                provider.call(Request::call(id, "generate", params)),
            )
            .await
            {
                Ok(response) => response,

                Err(_) => {
                    provider.restart().await?;
                    Err(Bridge::expired())
                }
            }
        }

        "host/model_stream" => {
            selected_workspace(&params, &workspace)?;
            let (cancel, cancelled) = tokio::sync::oneshot::channel();
            let mut cancel_guard = ModelStreamCancellation(Some(cancel));
            let response = tokio::spawn(async move {
                let mut model = model.lock().await;
                let Some(provider) = model.as_mut() else {
                    return Ok(Response::fail(
                        id,
                        -32000,
                        "No intelligence provider is available.",
                    ));
                };

                let mut notes = Vec::new();
                let mut expired = false;
                let (handoff, mut pending) = tokio::sync::oneshot::channel();
                let mut handoff = Some(handoff);
                let mut frontend = None;
                let allowed = params["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
                    .collect::<BTreeSet<_>>();
                let result = {
                    let note = |note: Request| {
                        if let Some(event) = stream_event(note.clone()) {
                            let _ = events.send(Request::Note {
                                jsonrpc: "2.0".into(),
                                method: "event".into(),
                                params: serde_json::json!({"callId":id,"event":event}),
                            });
                        }

                        notes.push(note);
                        std::future::ready(Ok(()))
                    };

                    let call = |request| {
                        let event = match request {
                            Request::Call { method, params, .. }
                                if method == "host/tool"
                                    && params["name"]
                                        .as_str()
                                        .is_some_and(|name| allowed.contains(name)) =>
                            {
                                Ok(Event::Tool {
                                    name: params["name"].as_str().unwrap().to_owned(),
                                    args: params["args"].clone(),
                                    id: None,
                                    thought_signature: None,
                                })
                            }

                            _ => Err(krabb_core::Error::Protocol(
                                concat!(
                                    "Provider requested an undeclared frontend tool ",
                                    "or unsupported host method."
                                )
                                .into(),
                            )),
                        };

                        if let Some(handoff) = handoff.take() {
                            let _ = handoff.send(event);
                        }

                        std::future::pending::<krabb_core::Result<Response>>()
                    };

                    let generation =
                        provider.call_full_async(Request::call(id, "generate", params), note, call);

                    tokio::pin!(generation);

                    tokio::select! {
                        result = &mut generation => Some(result),
                        _ = cancelled => None,

                        _ = tokio::time::sleep(bridge.limit) => {
                            expired = true;
                            None
                        }

                        event = &mut pending => {
                            frontend = Some(event.map_err(|_| krabb_core::Error::Protocol(
                                "Provider tool handoff closed.".into(),
                            ))?);
                            None
                        }
                    }
                };

                let response = match result {
                    Some(response) => response?,

                    None => {
                        provider.restart().await?;

                        if expired {
                            return Err(Bridge::expired());
                        }

                        if let Some(event) = frontend {
                            let mut normalized =
                                notes.into_iter().filter_map(stream_event).collect::<Vec<_>>();
                            normalized.push(event?);
                            let text = normalized
                                .iter()
                                .filter_map(|event| match event {
                                    Event::Text { text } => Some(text.as_str()),
                                    _ => None,
                                })
                                .collect::<String>();

                            let reply = ModelReply {
                                timing: None,
                                text,
                                stop: "tool_calls".into(),
                                input: None,
                                output: None,
                                context_usage: None,
                                events: normalized,
                            };

                            return Ok(Response::ok(id, serde_json::to_value(reply)?));
                        }

                        return Err(krabb_core::Error::Protocol(
                            "The model request was cancelled.".into(),
                        ));
                    }
                };

                if let Some(value) = response.result.as_ref()
                    && let Ok(mut reply) = serde_json::from_value::<ModelReply>(value.clone())
                {
                    reply.events.extend(notes.into_iter().filter_map(stream_event));
                    return Ok(Response::ok(id, serde_json::to_value(reply)?));
                }

                Ok(response)
            })
            .await
            .map_err(|error| {
                krabb_core::Error::Protocol(format!("Model stream task failed: {error}."))
            })??;
            cancel_guard.0 = None;

            Ok(response)
        }

        "host/tool" => {
            agent_tool_request(id, &params, &tools, &workspace, bridge.approval, |question| {
                confirm_action(false, json, question).unwrap_or(false)
            })
            .await
        }

        _ => Ok(Response::fail(id, -32601, "Agent host method not found.")),
    }
}

pub(crate) struct ModelStreamCancellation(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for ModelStreamCancellation {
    fn drop(&mut self) {
        if let Some(cancel) = self.0.take() {
            let _ = cancel.send(());
        }
    }
}

pub(crate) fn selected_workspace(
    params: &serde_json::Value,
    configured: &Path,
) -> std::result::Result<PathBuf, krabb_core::Error> {
    let Some(value) = params["workspace"].as_str() else {
        return Ok(configured.to_owned());
    };

    let workspace = std::fs::canonicalize(value).map_err(|error| {
        krabb_core::Error::Denied(format!("The selected workspace is unavailable: {error}."))
    })?;

    if !workspace.is_dir() {
        return Err(krabb_core::Error::Denied("The selected workspace is not a directory.".into()));
    }

    if !workspace.starts_with(configured) {
        return Err(krabb_core::Error::Denied(
            "The selected workspace leaves the configured root.".into(),
        ));
    }

    Ok(workspace)
}

pub(crate) fn approval_arguments(
    name: &str,
    args: &serde_json::Value,
) -> Result<String, PluginError> {
    let arguments = args.to_string();

    let limit = if matches!(name, "schedule_propose" | "schedule_update" | routing::NAME) {
        APPROVAL_SCHEDULE_LIMIT
    } else {
        APPROVAL_ARGUMENT_LIMIT
    };

    if arguments.len() > limit {
        return Err(format!(
            "Tool arguments exceed the approval display limit of {limit} JSON bytes.\nShorten \
            the request so it can be reviewed in full."
        )
        .into());
    }

    Ok(arguments)
}

pub(crate) async fn agent_tool_request(
    id: u64,
    params: &serde_json::Value,
    tools: &AsyncMutex<Option<Process>>,
    workspace: &Path,
    approval_mode: ApprovalMode,
    confirm: impl FnOnce(&str) -> bool,
) -> krabb_core::Result<Response> {
    let mut tools = tools.lock().await;

    let Some(tool_process) = tools.as_mut() else {
        return Ok(Response::fail(id, -32000, "The tools plugin is not installed."));
    };

    let workspace = selected_workspace(params, workspace)?;

    let Some(name) = params["name"].as_str() else {
        return Ok(Response::fail(id, -32602, "A tool name is required."));
    };

    let (name, mut args) = aliases::current(name, params["args"].clone(), |_| false);
    let name = name.as_str();

    let Some(values) = args.as_object_mut() else {
        return Ok(Response::fail(id, -32602, "Tool arguments must be an object."));
    };

    values.remove("approve");
    values.insert("workspace".into(), serde_json::Value::String(workspace.display().to_string()));

    let changes = mutating(name, &args);

    let approved = match (changes, approval_mode) {
        (false, _) | (true, ApprovalMode::Off) => false,
        (true, ApprovalMode::Auto) => true,

        (true, ApprovalMode::Prompt) => {
            let arguments = match approval_arguments(name, &args) {
                Ok(arguments) => arguments,
                Err(error) => return Ok(Response::fail(id, -32000, error.to_string())),
            };

            let question =
                format!("Krabb requests approval to run {name:?} with {arguments}.\nApprove?");

            if question.len() > APPROVAL_TEXT_LIMIT {
                return Ok(Response::fail(
                    id,
                    -32000,
                    "The request exceeds the approval display limit.",
                ));
            }

            confirm(&question)
        }
    };

    if changes && !approved {
        return Ok(Response::fail(id, -32000, "Krabb approval blocked the tool."));
    }

    args.as_object_mut().unwrap().insert("approve".into(), serde_json::Value::Bool(approved));
    tool_process.call(Request::call(id, name, args)).await
}

/// Whether a person can answer prompts: not in CI, with a terminal on stdin and stderr.
pub(crate) fn interactive() -> bool {
    std::env::var_os("CI").is_none()
        && std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
}

pub(crate) fn confirm_action(yes: bool, json: bool, question: &str) -> Result<bool, PluginError> {
    if yes {
        return Ok(true);
    }

    if json || !interactive() {
        return Err(format!("{question}\nUse --yes to continue without a prompt.").into());
    }

    eprint!("{question} [y/N] ");

    std::io::stderr().flush()?;

    let mut answer = String::new();

    std::io::stdin().read_line(&mut answer)?;

    Ok(matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

pub(crate) fn print_cancelled(action: &str, json: bool) {
    if json {
        println!("{}", serde_json::json!({"action": action, "status": "cancelled"}));
    } else {
        println!("{} cancelled.", action.trim_end_matches(['.', '!', '?']));
    }
}
