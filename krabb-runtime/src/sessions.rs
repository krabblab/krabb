//! Implements the `krabb session` and `krabb delivery` command families.

use crate::*;

pub(crate) async fn session_command(
    args: &[String],
    root: &Path,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let argv = std::iter::once("session".to_owned()).chain(args.iter().cloned());

    let cli = match SessionCli::try_parse_from(argv) {
        Ok(cli) => cli,

        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp
                    | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            ) =>
        {
            print!("{error}");
            return Ok(());
        }

        Err(error) => return Err(error.into()),
    };

    let SessionCli { json: command_json, debug, verbose, command } = cli;
    let _ = (debug, verbose);

    session_at_json(command, root, json || command_json).await
}

pub(crate) async fn delivery_command(
    args: &[String],
    root: &Path,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let argv = std::iter::once("delivery".to_owned()).chain(args.iter().cloned());
    let cli = match DeliveryCli::try_parse_from(argv) {
        Ok(cli) => cli,

        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp
                    | clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            ) =>
        {
            print!("{error}");
            return Ok(());
        }

        Err(error) => return Err(error.into()),
    };

    let DeliveryCli { json: command_json, debug, verbose, command } = cli;
    let _ = (debug, verbose);

    delivery_at_json(command, root, json || command_json).await
}

#[cfg(test)]
pub(crate) async fn delivery_at(
    command: DeliveryCommand,
    root: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    delivery_at_json(command, root, false).await
}

pub(crate) async fn delivery_at_json(
    command: DeliveryCommand,
    root: &Path,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match command {
        DeliveryCommand::List(output) => {
            validate_page(output.page)?;

            let value = if let Some(value) =
                control_at("delivery.list", serde_json::json!({}), root).await?
            {
                value
            } else {
                let _offline_lock = offline_lock(root)?;
                let store = sessions_at(root)?;
                serde_json::json!({
                    "items": store.outbox.iter().map(ipc::delivery).collect::<Vec<_>>()
                })
            };

            if output.json || json {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                println!("{}", format_delivery_list(&value, output.page)?);
            }
        }

        DeliveryCommand::Retry(args) => {
            if !confirm_action(
                args.yes,
                json,
                &format!("Retry delivery {}? It may be sent more than once.", args.id),
            )? {
                print_cancelled("Retry", json);
                return Ok(());
            }

            let id = args.id.clone();

            if let Some(value) =
                control_at("delivery.retry", serde_json::json!({"id": id, "yes": true}), root)
                    .await?
            {
                info!(
                    operation = "delivery.retry",
                    source = "daemon",
                    "Delivery queued for retry."
                );

                if json {
                    println!("{}", serde_json::to_string_pretty(&value)?);
                } else {
                    println!(
                        "Queued delivery {} for retry.",
                        value["id"].as_str().unwrap_or_default()
                    );
                }

                return Ok(());
            }

            let _offline_lock = offline_lock(root)?;
            let mut store = sessions_at(root)?;
            store.retry_delivery(&args.id)?;
            info!(operation = "delivery.retry", source = "offline", "Delivery queued for retry.");

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "action": "retry",
                        "id": args.id,
                        "status": "queued"
                    }))?
                );
            } else {
                println!("Queued delivery {} for retry.", args.id);
            }
        }

        DeliveryCommand::Drop(args) => delivery::drop(args, root, json).await?,
    }

    Ok(())
}

#[cfg(test)]
pub(crate) async fn session_at(
    command: SessionCommand,
    root: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    session_at_json(command, root, false).await
}

pub(crate) async fn session_at_json(
    command: SessionCommand,
    root: &Path,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match command {
        SessionCommand::New(args) => {
            let id = args.id.clone();
            let model = session_model(args.model.clone(), root)?;

            if let Some(value) =
                control_at("session.new", serde_json::json!({"id": id, "model": model}), root)
                    .await?
            {
                info!(operation = "session.create", source = "daemon", "Session created.");

                if json {
                    println!("{}", serde_json::to_string_pretty(&value)?);
                } else {
                    println!("Created session {}.", value["id"].as_str().unwrap_or_default());
                }

                return Ok(());
            }

            local_session_mode(SessionCommand::New(args), root, json)?;
        }

        SessionCommand::List(output) => {
            validate_page(output.page)?;

            let value = if let Some(value) =
                control_at("session.list", serde_json::json!({}), root).await?
            {
                value
            } else {
                let _offline_lock = offline_lock(root)?;
                let store = sessions_at(root)?;
                serde_json::json!({
                    "items": store.sessions.values().map(ipc::summary).collect::<Vec<_>>()
                })
            };

            if output.json || json {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                println!("{}", format_session_list(&value, output.page)?);
            }
        }

        SessionCommand::Show(args) => {
            let value = if let Some(value) =
                control_at("session.get", serde_json::json!({"id": args.id}), root).await?
            {
                value
            } else {
                let _offline_lock = offline_lock(root)?;
                let store = sessions_at(root)?;
                ipc::detail(store.sessions.get(&args.id).ok_or("Session was not found.")?)
            };

            if json {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                println!("{}", session_text(&value));
            }
        }

        SessionCommand::Fork(args) => {
            let source = args.source.clone();
            let target = args.target.clone();

            if let Some(value) = control_at(
                "session.fork",
                serde_json::json!({"source": source, "target": target}),
                root,
            )
            .await?
            {
                info!(operation = "session.fork", source = "daemon", "Session forked.");

                if json {
                    println!("{}", serde_json::to_string_pretty(&value)?);
                } else {
                    println!("Forked session {}.", value["id"].as_str().unwrap_or_default());
                }

                return Ok(());
            }

            local_session_mode(SessionCommand::Fork(args), root, json)?;
        }

        SessionCommand::Model(args) => {
            let id = args.id.clone();
            let model = args.model.clone();

            if let Some(value) =
                control_at("session.model", serde_json::json!({"id": id, "model": model}), root)
                    .await?
            {
                info!(operation = "session.model", source = "daemon", "Session model changed.");

                if json {
                    println!("{}", serde_json::to_string_pretty(&value)?);
                } else if args.model == "inherit" {
                    println!("Session {} now follows its channel's model.", args.id);
                } else {
                    println!(
                        "Session {} now uses {}.",
                        value["id"].as_str().unwrap_or_default(),
                        value["model"].as_str().unwrap_or_default()
                    );
                }

                return Ok(());
            }

            local_session_mode(SessionCommand::Model(args), root, json)?;
        }

        SessionCommand::Cancel(args) => {
            let id = args.id.clone();

            if let Some(value) =
                control_at("session.cancel", serde_json::json!({"id": id}), root).await?
            {
                info!(operation = "session.cancel", source = "daemon", "Session cancelled.");

                if json {
                    println!("{}", serde_json::to_string_pretty(&value)?);
                } else {
                    println!("Cancelled session {}.", value["id"].as_str().unwrap_or_default());
                }

                return Ok(());
            }

            local_session_mode(SessionCommand::Cancel(args), root, json)?;
        }

        SessionCommand::Delete(mut args) => {
            if !confirm_action(args.yes, json, &format!("Delete session {}?", args.id))? {
                print_cancelled("Delete", json);
                return Ok(());
            }

            args.yes = true;

            let id = args.id.clone();

            if let Some(value) =
                control_at("session.delete", serde_json::json!({"id": id}), root).await?
            {
                if value["worktree"]["status"] == "pending" {
                    warn!(
                        operation = "session.delete",
                        source = "daemon",
                        "Session deleted, but worktree cleanup is pending."
                    );
                } else {
                    info!(operation = "session.delete", source = "daemon", "Session deleted.");
                }

                if json {
                    println!("{}", serde_json::to_string_pretty(&value)?);
                } else if value["worktree"]["status"] == "pending" {
                    println!(
                        "{}",
                        sentence(format!(
                            "Deleted session {}, but worktree cleanup is pending: {}",
                            value["id"].as_str().unwrap_or_default(),
                            value["worktree"]["error"]
                                .as_str()
                                .unwrap_or("Retry at daemon startup.")
                        ))
                    );
                } else {
                    println!("Deleted session {}.", value["id"].as_str().unwrap_or_default());
                }

                return Ok(());
            }

            local_session_mode(SessionCommand::Delete(args), root, json)?;
        }
    }

    Ok(())
}

pub(crate) fn local_session(
    command: SessionCommand,
    root: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    local_session_json(command, root, false)
}

pub(crate) fn local_session_mode(
    command: SessionCommand,
    root: &Path,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if json { local_session_json(command, root, true) } else { local_session(command, root) }
}

pub(crate) fn local_session_json(
    command: SessionCommand,
    root: &Path,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    std::fs::create_dir_all(root)?;
    let _offline_lock = offline_lock(root)?;
    let mut store = sessions_at(root)?;

    match command {
        SessionCommand::New(args) => {
            let id = args.id.clone();
            let model = session_model(args.model, root)?;
            store.create(id.clone(), model.clone())?;
            info!(operation = "session.create", source = "offline", "Session created.");

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "action": "create",
                        "id": id,
                        "model": model
                    }))?
                );
            } else {
                println!("Created session {}.", args.id);
            }
        }

        SessionCommand::List(output) => {
            validate_page(output.page)?;

            let items = store
                .sessions
                .values()
                .filter(|session| !session.is_work())
                .map(ipc::summary)
                .collect::<Vec<_>>();

            let value = serde_json::json!({"items": items});

            if output.json || json {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                println!("{}", format_session_list(&value, output.page)?);
            }
        }

        SessionCommand::Show(args) => {
            let session = store.sessions.get(&args.id).ok_or("Session was not found.")?;
            let value = ipc::detail(session);

            if json {
                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                println!("{}", session_text(&value));
            }
        }

        SessionCommand::Fork(args) => {
            store.fork(&args.source, &args.target)?;
            info!(operation = "session.fork", source = "offline", "Session forked.");

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "action": "fork",
                        "id": args.target,
                        "source": args.source
                    }))?
                );
            } else {
                println!("Forked session {}.", args.target);
            }
        }

        SessionCommand::Model(args) => {
            store.set_model(&args.id, &args.model)?;
            info!(operation = "session.model", source = "offline", "Session model changed.");

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "action": "model",
                        "id": args.id,
                        "model": args.model,
                        "pinned": store.sessions[&args.id].pinned,
                    }))?
                );
            } else if args.model == "inherit" {
                println!("Session {} now follows its channel's model.", args.id);
            } else {
                println!("Session {} now uses {}.", args.id, args.model);
            }
        }

        SessionCommand::Cancel(args) => {
            store.cancel(&args.id)?;
            info!(operation = "session.cancel", source = "offline", "Session cancelled.");

            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "action": "cancel",
                        "id": args.id,
                        "status": "cancelled"
                    }))?
                );
            } else {
                println!("Cancelled session {}.", args.id);
            }
        }

        SessionCommand::Delete(args) => {
            if !confirm_action(args.yes, json, &format!("Delete session {}?", args.id))? {
                print_cancelled("Delete", json);
                return Ok(());
            }

            store.remove(&args.id)?;

            let worktree_error = state::remove_worktree(root, &args.id).err();

            if worktree_error.is_some() {
                warn!(
                    operation = "session.delete",
                    "Session deleted, but worktree cleanup is pending."
                );
            } else {
                info!(operation = "session.delete", source = "offline", "Session deleted.");
            }

            if json {
                let mut value = serde_json::json!({
                    "action": "delete",
                    "id": args.id,
                    "status": "deleted"
                });

                if let Some(error) = worktree_error {
                    value["worktree"] = serde_json::json!({
                        "status": "pending",
                        "error": error.to_string()
                    });
                }

                println!("{}", serde_json::to_string_pretty(&value)?);
            } else {
                if let Some(error) = worktree_error {
                    println!(
                        "{}",
                        sentence(format!(
                            "Session {} was deleted, but its worktree could not be reclaimed: {}",
                            args.id, error
                        ))
                    );
                }

                println!("Deleted session {}.", args.id);
            }
        }
    }

    Ok(())
}

pub(crate) fn session_text(value: &serde_json::Value) -> String {
    let mut lines = vec![
        format!("Session: {}", value["id"].as_str().unwrap_or("unknown")),
        format!("Intelligence: {}", value["model"].as_str().unwrap_or("unknown")),
        format!("Status: {}", value["status"].as_str().unwrap_or("unknown")),
    ];

    if let Some(workspace) = value["workspace"].as_str() {
        lines.push(format!("Workspace: {workspace}"));
    }

    lines.push("Transcript:".into());
    let messages = value["messages"].as_array();

    if messages.is_none_or(Vec::is_empty) {
        lines.push("(empty)".into());
    } else {
        let entries = messages
            .into_iter()
            .flatten()
            .map(|message| {
                let role = message["role"].as_str().unwrap_or("unknown");
                let content = message["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|item| {
                        item["text"]
                            .as_str()
                            .or_else(|| item["alt"].as_str())
                            .or_else(|| item["name"].as_str())
                            .unwrap_or_else(|| item["kind"].as_str().unwrap_or("attachment"))
                    })
                    .collect::<Vec<_>>()
                    .join(" ");

                format!("{role}: {content}")
            })
            .collect::<Vec<_>>();

        lines.push(format_tree_list(&entries));
    }

    lines.join("\n")
}
