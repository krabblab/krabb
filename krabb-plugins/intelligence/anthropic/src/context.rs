use krabb_core::{
    context::{CompactReply, CompactRequest, Generation, Profile, Window},
    types::ModelRequest,
};

use serde_json::{Value, json};

const BETA: &str = "compact-2026-09-04";

pub(super) async fn profile(
    client: &reqwest::Client,
    model: &str,
    key: &str,
    base: &str,
) -> krabb_core::Result<Profile> {
    let base = super::validate_base(base)?;
    let mut url = reqwest::Url::parse(&format!("{base}/v1/models/"))
        .map_err(|_| krabb_core::Error::Denied("Anthropic model URL is invalid.".into()))?;
    url.path_segments_mut().unwrap().pop_if_empty().push(model);
    let response = client
        .get(url)
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", BETA)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| krabb_core::Error::Denied("Anthropic model discovery failed.".into()))?;
    let body = read(response).await?;

    metadata(&body)
}

fn metadata(body: &Value) -> krabb_core::Result<Profile> {
    let tokens = body["max_input_tokens"].as_u64().ok_or_else(|| {
        krabb_core::Error::Protocol("Anthropic model metadata omitted its input limit.".into())
    })?;
    let maximum = body["max_tokens"].as_u64().unwrap_or(4096);
    let window = Window { tokens, output: maximum.min(4096).min(tokens / 2) };
    window.input()?;

    Ok(Profile {
        window: Some(window),
        compact: body["capabilities"]["compaction"]["supported"] == true,
        search: body["capabilities"]["server_tools"]["web_search"]["supported"] == true,
    })
}

pub(super) async fn compact(
    client: &reqwest::Client,
    request: CompactRequest,
    key: &str,
    base: &str,
) -> krabb_core::Result<CompactReply> {
    let input = ModelRequest {
        model: super::model::resolve(&request.model),
        workspace: None,
        generation: Some(Generation { output: request.max_tokens, summary: true, search: false }),
        messages: request.messages,
        stream: false,
        tools: vec![],
    };

    let (system, messages) = super::messages(&input)?;
    let base = super::validate_base(base)?;
    let response = client
        .post(format!("{base}/v1/messages"))
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", BETA)
        .json(&json!({"model":input.model,"system":system,"messages":messages,
            "max_tokens":request.output.unwrap_or(request.max_tokens),"thinking":{"type":"disabled"},
            "compaction":{"type":"summarize", "instructions":format!("Keep the summary within {} tokens.", request.max_tokens)}}))
        .send()
        .await
        .map_err(|_| krabb_core::Error::Denied("Anthropic compaction failed.".into()))?;
    let body = read(response).await?;
    let text = body["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "compaction")
        .filter_map(|block| block["content"].as_str())
        .collect::<String>();

    if body["stop_reason"] != "compaction" || text.trim().is_empty() {
        return Err(krabb_core::Error::Protocol(
            "Anthropic returned no completed compaction summary.".into(),
        ));
    }

    // Normalize readable summary text for portability across model/provider changes.
    // No signed native block is reconstructed or sent back with modified contents.
    Ok(CompactReply { text })
}

async fn read(response: reqwest::Response) -> krabb_core::Result<Value> {
    if !response.status().is_success() {
        return Err(krabb_core::Error::Denied(format!(
            "Anthropic context request was rejected with {}.",
            response.status(),
        )));
    }

    Ok(serde_json::from_str(&super::read(response, "Anthropic").await?)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn discovers_capabilities_and_normalizes_native_compaction_over_http() {
        let metadata = json!({"max_input_tokens":200000,"max_tokens":64000,
            "capabilities":{"compaction":{"supported":true},"server_tools":{"web_search":{"supported":true}}}});
        let compacted = json!({"stop_reason":"compaction","content":[{"type":"compaction","content":"User prefers tea.","signature":"private-signature"}]});
        let (base, server) = krabb_test::http::responses(vec![
            (200, metadata.to_string()),
            (200, compacted.to_string()),
        ])
        .await;
        let client = reqwest::Client::new();
        let profile = profile(&client, "claude-fixture", "fixture", &base).await.unwrap();

        assert!(profile.compact && profile.search);

        let request: CompactRequest = serde_json::from_value(
            json!({"model":"claude-fixture","messages":[],"max_tokens":512,"output":4096}),
        )
        .unwrap();
        let reply = compact(&client, request, "fixture", &base).await.unwrap();

        assert_eq!(reply.text, "User prefers tea.");
        assert!(!serde_json::to_string(&reply).unwrap().contains("signature"));

        let requests = server.await.unwrap();

        assert!(requests[0].headers.starts_with("GET /v1/models/claude-fixture "));
        assert!(requests[1].headers.contains(BETA));
        assert_eq!(requests[1].body["max_tokens"], 4096);
        assert_eq!(requests[1].body["thinking"]["type"], "disabled");
        assert!(requests[1].body["compaction"]["instructions"].as_str().unwrap().contains("512"));
    }

    #[test]
    fn normalizes_model_limits_and_capabilities() {
        let value = json!({"max_input_tokens":200000,"max_tokens":64000,
            "capabilities":{"compaction":{"supported":true},
                "server_tools":{"web_search":{"supported":true}}}});
        let profile = metadata(&value).unwrap();

        assert_eq!(profile.window, Some(Window { tokens: 200000, output: 4096 }));
        assert!(profile.compact && profile.search);
        assert!(metadata(&json!({})).is_err());
        assert!(metadata(&json!({"max_input_tokens":0})).is_err());
        assert!(!metadata(&json!({"max_input_tokens":8192})).unwrap().search);
    }
}
