use krabb_core::{
    plugin::Emitter,
    types::{Content, Message, ModelRequest, Role, ToolSpec},
};

#[tokio::test]
async fn native_search_dispatches_to_responses_for_both_transports() {
    for streaming in [false, true] {
        let (base, server) = krabb_test::http::responses(vec![(403, "{}".into())]).await;

        let input = ModelRequest {
            generation: Some(krabb_core::context::Generation {
                output: 2048,
                search: true,
                summary: false,
            }),
            model: "default".into(),
            workspace: None,
            stream: streaming,
            tools: vec![],
            messages: vec![Message {
                id: "1".into(),
                session: "fixture".into(),
                role: Role::User,
                sender: None,
                content: vec![Content::Text { text: "Find the release notes.".into() }],
            }],
        };

        let (output, _events) = tokio::sync::mpsc::channel(16);
        let mut emitter = Emitter::new(output);
        let client = reqwest::Client::new();

        let result = if streaming {
            crate::stream(&client, 1, input, "fixture", &base, &mut emitter).await
        } else {
            crate::complete(&client, 1, input, "fixture", &base).await
        };

        assert!(result.unwrap_err().to_string().contains("403"));

        let requests = server.await.unwrap();

        assert!(requests[0].headers.starts_with("POST /responses "));
        assert_eq!(requests[0].body["model"], "gpt-6-luna");
        assert_eq!(requests[0].body["stream"], streaming);
        assert_eq!(requests[0].body["tools"][0]["type"], "web_search");
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

            let result = if stream {
                crate::stream(&client, 1, input, "fixture-key", &base, &mut emitter).await
            } else {
                crate::complete(&client, 1, input, "fixture-key", &base).await
            };

            assert!(result.is_err());

            let request = server.await.unwrap();

            assert_eq!(
                request["model"],
                if model == "explicit" { "explicit" } else { "gpt-6-luna" }
            );

            assert_eq!(request["stream"], stream);
            assert_eq!(request["tools"].as_array().unwrap().len(), 1);

            assert_eq!(
                request["reasoning_effort"],
                if model == "explicit" { serde_json::Value::Null } else { "none".into() }
            );
        }
    }
}
