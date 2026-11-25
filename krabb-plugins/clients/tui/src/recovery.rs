use super::*;
use krabb_core::task::State;
use serde::Deserialize;

const LOOKUPS: usize = 8;

pub(super) async fn recent<C, F>(
    home: &str,
    session: &str,
    messages: &[Message],
    host: C,
) -> krabb_core::Result<Option<Value>>
where
    C: Fn(String, String, Value) -> F,
    F: Future<Output = krabb_core::Result<Value>>,
{
    for request in messages
        .iter()
        .rev()
        .filter(|message| {
            message.role == Role::User && message.sender.as_deref() == Some(client::TUI)
        })
        .take(LOOKUPS)
    {
        if let Some(saved) = lookup(home, session, &request.id, &host).await? {
            return Ok(Some(saved));
        }
    }

    Ok(None)
}

#[derive(Deserialize)]
struct Receipt {
    found: bool,
    #[serde(default)]
    task: Option<Task>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    expired: bool,
}

#[derive(Deserialize, serde::Serialize)]
struct Task {
    id: String,
    state: State,
}

pub(super) async fn lookup<C, F>(
    home: &str,
    session: &str,
    request: &str,
    host: C,
) -> krabb_core::Result<Option<Value>>
where
    C: Fn(String, String, Value) -> F,
    F: Future<Output = krabb_core::Result<Value>>,
{
    let value = host(
        home.into(),
        "task.receipt".into(),
        serde_json::json!({
            "session":session,"client":client::TUI,"request":request,
        }),
    )
    .await?;

    let receipt: Receipt = serde_json::from_value(value)
        .map_err(|_| krabb_core::Error::Protocol("Invalid task receipt.".into()))?;

    if !receipt.found && receipt.task.is_none() && receipt.text.is_none() {
        return Ok(None);
    }

    let Some(task) = receipt
        .task
        .filter(|task| receipt.found && task.id.len() <= 128 && krabb_core::task::id(&task.id))
    else {
        return Err(krabb_core::Error::Protocol("Invalid task receipt identity.".into()));
    };

    if receipt.expired {
        if receipt.text.is_some() {
            return Err(krabb_core::Error::Protocol("Invalid expired task receipt.".into()));
        }

        return Ok(Some(serde_json::json!({"done":true,"recovered":true,"expired":true,
            "task":task,"text":""})));
    }

    if receipt.text.as_ref().is_some_and(|text| {
        text.len() > jsonl::MAX || !matches!(task.state, State::Reporting | State::Completed)
    }) {
        return Err(krabb_core::Error::Protocol("Invalid saved task result.".into()));
    }

    Ok(Some(serde_json::json!({"done":true,"recovered":true,
        "task":task,"text":receipt.text.unwrap_or_default()})))
}

/// Whether a recovered receipt tells more than the error it would replace: a failed or
/// cancelled task without a saved result keeps the original error.
pub(super) fn useful(saved: &Value) -> bool {
    saved["text"].as_str().is_some_and(|text| !text.is_empty())
        || !matches!(saved["task"]["state"].as_str(), Some("failed" | "cancelled"))
}

pub(super) fn notice(value: &Value) -> Option<String> {
    if value["recovered"] != true && value["replayed"] != true {
        return None;
    }

    let id = value["task"]["id"].as_str()?;
    let state = value["task"]["state"].as_str()?;

    if value["expired"] == true {
        return Some(format!(
            "Task {id} finished as {state}, but its saved result has expired.\nSend the request \
            again to run it anew."
        ));
    }

    Some(format!("Task {id} is {state}.\nThe saved request has been recovered."))
}

pub(super) async fn cleanup<C, F>(home: &str, session: &str, host: C) -> Option<String>
where
    C: Fn(String, String, Value) -> F,
    F: Future<Output = krabb_core::Result<Value>>,
{
    let result =
        host(home.into(), "session.cleanup".into(), serde_json::json!({"id":session})).await;

    let name = session.strip_prefix("tui-").unwrap_or(session);

    match result {
        Ok(value) if value["id"] == session && value["deleted"] == true => {
            if value["worktree"]["status"] == "pending" {
                Some("The conversation was deleted, and workspace cleanup is pending.".into())
            } else {
                None
            }
        }

        Ok(value) if value["id"] == session && value["deleted"] == false => {
            Some(format!("Conversation retained.\nReopen it with `krabb tui --session {name}`"))
        }

        _ => Some(format!(
            "Conversation cleanup could not be confirmed.\nIf it is still present, reopen it \
            with `krabb tui --session {name}`"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_failed_task_without_a_result_keeps_the_original_error() {
        let receipt =
            |state: &str, text: &str| json!({"task":{"id":"task-1","state":state},"text":text});

        assert!(!useful(&receipt("failed", "")));
        assert!(!useful(&receipt("cancelled", "")));
        assert!(useful(&receipt("running", "")));
        assert!(useful(&receipt("completed", "Done.")));
    }

    #[tokio::test]
    async fn recent_recovery_skips_controls_with_a_bounded_number_of_receipt_lookups() {
        let messages = (0..12)
            .map(|n| Message {
                id: format!("input-{n}"),
                session: "conversation".into(),
                role: Role::User,
                sender: Some(client::TUI.into()),
                content: vec![Content::Text { text: "pause the report".into() }],
            })
            .collect::<Vec<_>>();
        let queried = std::cell::RefCell::new(Vec::new());
        let host = |_: String, method: String, params: Value| {
            assert_eq!(method, "task.receipt");
            queried.borrow_mut().push(params["request"].as_str().unwrap().to_owned());
            std::future::ready(Ok(json!({"found":false})))
        };

        assert!(recent("home", "conversation", &messages, host).await.unwrap().is_none());
        assert_eq!(
            *queried.borrow(),
            (4..12).rev().map(|n| format!("input-{n}")).collect::<Vec<_>>()
        );
        queried.borrow_mut().clear();
        let host = |_: String, method: String, params: Value| {
            assert_eq!(method, "task.receipt");
            queried.borrow_mut().push(params["request"].as_str().unwrap().to_owned());
            std::future::ready(if params["request"] == "input-11" {
                Err(krabb_core::Error::Protocol("The daemon is unavailable.".into()))
            } else {
                Ok(json!({"found":false}))
            })
        };

        assert!(recent("home", "conversation", &messages, host).await.is_err());
        assert_eq!(*queried.borrow(), ["input-11"]);
    }

    async fn host(value: String, method: String, params: Value) -> krabb_core::Result<Value> {
        assert_eq!(method, "task.receipt");
        assert_eq!(
            params,
            json!({"session":"conversation","client":client::TUI,"request":"stable-1"})
        );
        Ok(serde_json::from_str(&value).unwrap())
    }

    #[tokio::test]
    async fn an_expired_receipt_is_reported_without_a_fabricated_result() {
        let expired = lookup(
            &json!({"found":true,"expired":true,"task":{"id":"task-1","state":"completed"}})
                .to_string(),
            "conversation",
            "stable-1",
            host,
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(expired["text"], "");
        assert!(notice(&expired).unwrap().contains("saved result has expired"));

        assert!(
            lookup(
                &json!({"found":true,"expired":true,"task":{"id":"task-1","state":"completed"},
                    "text":"Invented."})
                .to_string(),
                "conversation",
                "stable-1",
                host,
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn recovery_preserves_actual_state_and_rejects_fabricated_or_malformed_results() {
        let saved = lookup(
            &json!({"found":true,"task":{"id":"task-1","state":"completed"},
            "text":"The exact saved report."})
            .to_string(),
            "conversation",
            "stable-1",
            host,
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(saved["text"], "The exact saved report.");
        assert!(notice(&saved).unwrap().contains("completed"));

        let legacy = json!({"found":true,"task":{"id":"job-1","state":"completed"},"text":"Old."});

        assert!(lookup(&legacy.to_string(), "conversation", "stable-1", host).await.is_ok());

        assert!(
            lookup(&json!({"found":false}).to_string(), "conversation", "stable-1", host)
                .await
                .unwrap()
                .is_none()
        );

        for value in [
            json!(null),
            json!({"found":true}),
            json!({"found":false,"task":{"id":"task-1","state":"running"}}),
            json!({"found":true,"task":{"id":"task-1","state":"invented"}}),
            json!({"found":true,"task":{"id":"foreign-task","state":"completed"}}),
            json!({"found":true,"task":{"id":"task-","state":"completed"}}),
            json!({"found":true,"task":{"id":"task-1","state":"running"},"text":"Not verified."}),
        ] {
            assert!(lookup(&value.to_string(), "conversation", "stable-1", host).await.is_err());
        }
    }

    #[tokio::test]
    async fn cleanup_distinguishes_retention_deletion_and_unconfirmed_outcomes() {
        async fn host(value: String, method: String, params: Value) -> krabb_core::Result<Value> {
            assert_eq!(method, "session.cleanup");
            assert_eq!(params["id"], "tui-once-123");

            if value == "legacy" {
                return Err(krabb_core::Error::Denied("Method not found.".into()));
            }

            Ok(serde_json::from_str(&value).unwrap())
        }

        assert!(
            cleanup(&json!({"id":"tui-once-123","deleted":true}).to_string(), "tui-once-123", host)
                .await
                .is_none()
        );

        for value in [
            "legacy".into(),
            json!({"deleted":false}).to_string(),
            json!({"deleted":true,"id":"somewhere-else"}).to_string(),
            "null".into(),
        ] {
            let retained = cleanup(&value, "tui-once-123", host).await.unwrap();

            assert!(retained.contains("krabb tui --session once-123"));
            assert!(retained.contains("could not be confirmed"));
            assert!(!retained.contains("Conversation retained"));
        }

        let retained = cleanup(
            &json!({"id":"tui-once-123","deleted":false}).to_string(),
            "tui-once-123",
            host,
        )
        .await
        .unwrap();

        assert!(retained.contains("Conversation retained"));
        let pending = cleanup(
            &json!({"id":"tui-once-123","deleted":true,
            "worktree":{"status":"pending"}})
            .to_string(),
            "tui-once-123",
            host,
        )
        .await
        .unwrap();

        assert!(pending.contains("was deleted"));
        assert!(pending.contains("workspace cleanup is pending"));
        assert!(!pending.contains("reopen"));
    }
}
