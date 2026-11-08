use krabb_core::{
    plugin::Emitter,
    types::{Content, Message, ModelRequest, Role, ToolSpec},
};

#[tokio::test]
async fn configured_thinking_and_context_reach_streaming_and_plain_requests() {
    if !krabb_test::inbox::isolated(
        "defaults::configured_thinking_and_context_reach_streaming_and_plain_requests",
        &[("KRABB_OLLAMA_THINK", "true"), ("KRABB_OLLAMA_CONTEXT_LENGTH", "4096")],
    )
    .await
    {
        return;
    }

    let client =
        reqwest::Client::builder().timeout(std::time::Duration::from_secs(3)).build().unwrap();

    for stream in [false, true] {
        let (host, server) = krabb_test::http::capture().await;

        let input = ModelRequest {
            generation: Some(krabb_core::context::Generation {
                output: 512,
                summary: false,
                search: false,
            }),
            model: "qwen3.5:0.8b".into(),
            workspace: None,
            messages: vec![
                Message {
                    id: "old-user".into(),
                    session: "fixture".into(),
                    role: Role::User,
                    sender: None,
                    content: vec![Content::Text { text: "old history ".repeat(1000) }],
                },
                Message {
                    id: "old-assistant".into(),
                    session: "fixture".into(),
                    role: Role::Assistant,
                    sender: None,
                    content: vec![Content::Text { text: "old answer".into() }],
                },
                Message {
                    id: "current".into(),
                    session: "fixture".into(),
                    role: Role::User,
                    sender: None,
                    content: vec![Content::Text { text: "ping".into() }],
                },
            ],
            tools: vec![],
            stream,
        };

        let (output, _) = tokio::sync::mpsc::channel(4);
        let mut emitter = Emitter::new(output);
        let result = crate::generate_at(&client, 1, input.clone(), &host, &mut emitter).await;

        assert!(result.is_err());
        assert_eq!(input.messages.len(), 3);

        let body = server.await.unwrap();

        assert_eq!(body["model"], "qwen3.5:0.8b");
        assert_eq!(body["stream"], stream);
        assert_eq!(body["think"], true);
        assert_eq!(body["options"]["num_ctx"], 4096);
        assert_eq!(body["messages"].as_array().unwrap().len(), 3);
        assert_eq!(body["options"]["num_predict"], 512);
    }
}

#[tokio::test]
async fn default_models_reach_both_http_paths_without_fallback() {
    let client =
        reqwest::Client::builder().timeout(std::time::Duration::from_secs(3)).build().unwrap();

    for model in ["", "unset", "default", "explicit"] {
        for stream in [false, true] {
            let (base, server) = krabb_test::http::capture().await;

            let input = ModelRequest {
                generation: None,
                model: model.into(),
                workspace: None,
                stream,
                messages: vec![Message {
                    id: "1".into(),
                    session: "fixture".into(),
                    role: Role::User,
                    sender: None,
                    content: vec![Content::Text { text: "hello".into() }],
                }],
                tools: vec![ToolSpec {
                    name: "read".into(),
                    description: None,
                    schema: serde_json::json!({"type":"object"}),
                }],
            };

            let (output, _) = tokio::sync::mpsc::channel(4);
            let mut emitter = Emitter::new(output);
            let result = crate::generate_at(&client, 1, input, &base, &mut emitter).await;

            assert!(result.is_err());

            let request = server.await.unwrap();

            assert_eq!(
                request["model"],
                if model == "explicit" { "explicit" } else { "qwen3.5:4b" }
            );

            assert_eq!(request["stream"], stream);
            assert_eq!(request["think"], false);
            assert_eq!(request["tools"].as_array().unwrap().len(), 1);
        }
    }
}

#[test]
fn cloud_defaults_use_cloud_names_on_both_request_paths() {
    for stream in [false, true] {
        let input = ModelRequest {
            generation: None,
            model: "default".into(),
            workspace: None,
            messages: vec![],
            tools: vec![],
            stream,
        };

        let body = crate::request_body(&input, "https://ollama.com", stream).unwrap();

        assert_eq!(body["model"], "deepseek-v4.1-flash");
        assert_eq!(body["stream"], stream);
        assert_eq!(body["options"]["num_predict"], 4096);
    }
}
