use super::*;
use krabb_core::operation::{self, Query, Receipt, Spec, Start, State};

fn response(state: &str) -> String {
    json!({"jsonrpc":"2.0", "id":1,"result":{"id":"remote-task-1",
        "status":{"state":state,"message":{"parts":[{"text":"Saved report"}]}}}})
    .to_string()
}

#[tokio::test]
async fn an_operation_reuses_its_remote_handle_and_collects_results_without_another_start() {
    let metadata = card("MOCK_ENDPOINT", true);

    let (base, server) = mock_server(vec![
        ("application/json".into(), metadata.clone()),
        ("application/json".into(), response("TASK_STATE_WORKING")),
        ("application/json".into(), metadata.clone()),
        ("application/json".into(), response("TASK_STATE_COMPLETED")),
        ("application/json".into(), metadata),
        ("application/json".into(), response("TASK_STATE_COMPLETED")),
    ])
    .await;

    let config = A2aConfig { card: Some(base.clone()), ..Default::default() };
    let (output, _events) = tokio::sync::mpsc::channel(8);
    let mut emitter = krabb_core::plugin::Emitter::new(output);
    let client = client();

    let start = Start {
        id: "host-task-1-operation-1".into(),
        name: "delegate".into(),
        arguments: json!({"prompt":"Prepare the report"}),
    };

    let started = crate::operation::call(
        &client,
        1,
        operation::START,
        serde_json::to_value(start).unwrap(),
        &mut emitter,
        &config,
    )
    .await
    .unwrap();

    let started: Receipt = serde_json::from_value(started.result.unwrap()).unwrap();

    assert_eq!(started.state, State::Running);
    assert!(started.result.is_none());

    let query = Query { name: "delegate".into(), handle: started.handle.unwrap() };

    for method in [operation::INSPECT, operation::RESULT] {
        let result = crate::operation::call(
            &client,
            2,
            method,
            serde_json::to_value(&query).unwrap(),
            &mut emitter,
            &config,
        )
        .await
        .unwrap();

        let receipt: Receipt = serde_json::from_value(result.result.unwrap()).unwrap();

        assert_eq!(receipt.state, State::Completed);
        assert_eq!(receipt.handle.as_deref(), Some(query.handle.as_str()));
        assert_eq!(receipt.result.unwrap(), krabb_core::types::ToolResult::text("Saved report"));
    }

    let requests = server.await.unwrap();

    assert_eq!(requests.iter().filter(|request| request.starts_with("GET ")).count(), 3);

    let payloads = requests
        .iter()
        .filter_map(|request| request.split_once("\r\n\r\n"))
        .filter_map(|(_, body)| serde_json::from_str::<Value>(body).ok())
        .collect::<Vec<_>>();

    assert_eq!(payloads.iter().filter(|value| value["method"] == "SendMessage").count(), 1);
    assert_eq!(payloads.iter().filter(|value| value["method"] == "GetTask").count(), 2);

    let sent = payloads.iter().find(|value| value["method"] == "SendMessage").unwrap();

    assert_eq!(sent["params"]["message"]["messageId"], "host-task-1-operation-1");
    assert_eq!(sent["params"]["configuration"]["returnImmediately"], true);
}

#[tokio::test]
async fn cancellation_reports_the_backend_state_and_rejects_changed_endpoints() {
    for changed in [false, true] {
        let metadata = card("MOCK_ENDPOINT", false);
        let metadata = if changed { metadata.replace("/rpc", "/another-rpc") } else { metadata };

        let mut responses = vec![("application/json".into(), metadata)];

        if !changed {
            responses.push(("application/json".into(), response("TASK_STATE_WORKING")));
        }

        let (base, server) = mock_server(responses).await;

        let query = Query {
            name: "delegate".into(),
            handle: json!({
                "card":base, "endpoint":format!("{base}/rpc"), "task":"remote-task-1"
            })
            .to_string(),
        };

        let (output, _events) = tokio::sync::mpsc::channel(8);
        let mut emitter = krabb_core::plugin::Emitter::new(output);

        let result = crate::operation::call(
            &client(),
            1,
            operation::CANCEL,
            serde_json::to_value(query).unwrap(),
            &mut emitter,
            &A2aConfig::default(),
        )
        .await;

        if changed {
            assert!(result.unwrap_err().to_string().contains("endpoint has changed"));
        } else {
            let receipt: Receipt = serde_json::from_value(result.unwrap().result.unwrap()).unwrap();

            assert_eq!(receipt.state, State::Running);
            assert!(receipt.result.is_none());
        }

        let requests = server.await.unwrap();

        assert_eq!(
            requests.iter().filter(|request| request.contains("CancelTask")).count(),
            usize::from(!changed)
        );
    }
}

#[tokio::test]
async fn discovery_requires_a_card_unless_a_usable_default_is_configured() {
    let (output, _events) = tokio::sync::mpsc::channel(8);
    let mut emitter = krabb_core::plugin::Emitter::new(output);

    let configs = [
        (A2aConfig::default(), true),
        (A2aConfig { card: Some(" ".into()), ..Default::default() }, true),
        (A2aConfig { card: Some("http://remote.example/card".into()), ..Default::default() }, true),
        (
            A2aConfig { card: Some("https://agent.example/card".into()), ..Default::default() },
            false,
        ),
    ];

    for (config, required) in configs {
        let listed =
            crate::operation::call(&client(), 1, operation::LIST, json!({}), &mut emitter, &config)
                .await
                .unwrap();

        let spec: Spec =
            serde_json::from_value(listed.result.unwrap()["operations"][0].clone()).unwrap();

        spec.validate().unwrap();

        let fields = spec.input["required"].as_array().unwrap();

        assert!(fields.contains(&json!("prompt")));
        assert_eq!(fields.contains(&json!("card")), required);
    }
}

#[tokio::test]
async fn discovery_declares_cancellation_and_refuses_unsupported_methods() {
    let (output, _events) = tokio::sync::mpsc::channel(8);
    let mut emitter = krabb_core::plugin::Emitter::new(output);
    let config = A2aConfig::default();

    let listed =
        crate::operation::call(&client(), 1, operation::LIST, json!({}), &mut emitter, &config)
            .await
            .unwrap();

    let spec: Spec =
        serde_json::from_value(listed.result.unwrap()["operations"][0].clone()).unwrap();

    spec.validate().unwrap();

    assert!(spec.cancel);

    let unsupported =
        crate::operation::call(&client(), 2, "operation.wait", json!({}), &mut emitter, &config)
            .await
            .unwrap();

    assert_eq!(unsupported.error.unwrap().code, -32601);

    assert!(
        crate::operation::call(
            &client(),
            3,
            operation::START,
            json!({
                "id":"request", "name":"delegate", "arguments":{"prompt":"", "secret":"unexpected"}
            }),
            &mut emitter,
            &config
        )
        .await
        .is_err()
    );
}
