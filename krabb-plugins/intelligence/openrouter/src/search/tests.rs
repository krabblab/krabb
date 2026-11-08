use super::*;

use krabb_core::{
    context::Generation,
    types::{Content, Message, Role},
};

fn input() -> ModelRequest {
    ModelRequest {
        model: "author/model".into(),
        workspace: None,
        stream: false,
        generation: Some(Generation { output: 2048, summary: false, search: true }),
        tools: vec![],
        messages: vec![Message {
            id: "1".into(),
            session: "fixture".into(),
            role: Role::User,
            sender: None,
            content: vec![Content::Text { text: "Find the latest release.".into() }],
        }],
    }
}

fn discovery() -> Value {
    json!({"data":{"endpoints":[
        {"tag":"native/provider", "context_length":32768, "max_completion_tokens":2048, "native_tools":{"openrouter:web_search":{"type":"web_search"}}},
        {"tag":"external/provider", "context_length":131072, "native_tools":{}},
        {"tag":"invalid", "context_length":0, "native_tools":{"openrouter:web_search":{}}},
    ]}})
}

#[tokio::test]
async fn native_search_pins_advertised_endpoints_and_preserves_citations() {
    for streaming in [false, true] {
        let message = json!({"content":"Released.", "annotations":[{"type":"url_citation","url_citation":{"url":"https://example.com/release", "title":"Release"}}]});
        let answer = if streaming {
            format!("data: {}\n\ndata: [DONE]\n\n", json!({"choices":[{"delta":message}]}))
        } else {
            json!({"choices":[{"message":message, "finish_reason":"stop"}]}).to_string()
        };

        let (base, server) =
            krabb_test::http::responses(vec![(200, discovery().to_string()), (200, answer)]).await;
        let (output, _events) = tokio::sync::mpsc::channel(16);
        let mut emitter = Emitter::new(output);
        let client = reqwest::Client::new();
        let reply = if streaming {
            super::super::stream(&client, 1, input(), "fixture", &base, &mut emitter).await
        } else {
            complete(&client, 1, input(), "fixture", &base).await
        }
        .unwrap()
        .unwrap();
        let parsed: ModelReply = serde_json::from_value(reply.result.unwrap()).unwrap();

        assert_eq!(parsed.text, "Released. [Release](https://example.com/release)");
        assert!(parsed.events.is_empty());

        let requests = server.await.unwrap();

        assert!(requests[0].headers.starts_with("GET /models/author/model/endpoints "));
        assert_eq!(requests[1].body["provider"]["only"], json!(["native/provider"]));
        assert_eq!(requests[1].body["provider"]["allow_fallbacks"], false);
        assert_eq!(requests[1].body["tools"][0]["parameters"]["engine"], "native");
        assert_eq!(requests[1].body["plugins"], json!([{"id":"web", "enabled":false}]));
    }
}

#[tokio::test]
async fn unavailable_native_search_is_not_emulated() {
    for response in [
        (200, json!({"data":{"endpoints":[]}}).to_string()),
        (403, "{}".into()),
        (404, "{}".into()),
    ] {
        let (base, server) = krabb_test::http::responses(vec![response]).await;
        let input = input();
        let mut body = request_body(&input, false).unwrap();
        configure(&reqwest::Client::new(), &input, &mut body, "fixture", &base).await.unwrap();

        assert_eq!(body["tools"], json!([]));
        assert!(body.get("provider").is_none());
        server.await.unwrap();
    }

    let mut input = input();

    input.generation.as_mut().unwrap().summary = true;

    let mut body = request_body(&input, false).unwrap();

    configure(&reqwest::Client::new(), &input, &mut body, "fixture", "invalid").await.unwrap();

    assert_eq!(body["tools"], json!([]));

    assert!(
        citations(&json!({"annotations":[{"url_citation":{"url":"javascript:bad"}}]})).is_empty()
    );
}

#[tokio::test]
async fn discovery_normalizes_limits_and_fails_on_transport_or_malformed_data() {
    let (base, server) = krabb_test::http::responses(vec![
        (200, json!({"data":{"context_length":131072}}).to_string()),
        (200, discovery().to_string()),
    ])
    .await;

    let profile =
        context::profile(&reqwest::Client::new(), "author/model", "fixture", &base).await.unwrap();

    assert!(profile.search);
    assert_eq!(profile.window, Some(Window { tokens: 32768, output: 2048 }));

    server.await.unwrap();

    for response in [(500, "{}".into()), (200, "invalid".into())] {
        let (base, server) = krabb_test::http::responses(vec![response]).await;

        assert!(
            endpoints(&reqwest::Client::new(), "author/model", "fixture", &base).await.is_err()
        );

        server.await.unwrap();
    }

    assert!(
        endpoints(&reqwest::Client::new(), "unknown", "fixture", "invalid")
            .await
            .unwrap()
            .is_empty()
    );

    assert!(
        endpoints(&reqwest::Client::new(), "author/model", "fixture", "invalid").await.is_err()
    );
}
