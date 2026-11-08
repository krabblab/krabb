use krabb_core::{
    context::{Profile, Window},
    types::ModelRequest,
};

pub(super) async fn profile(
    client: &reqwest::Client,
    input: &ModelRequest,
    key: &str,
    base: &str,
) -> krabb_core::Result<Profile> {
    let model = super::model_name(input)?;
    let response = client
        .get(format!("{base}/models/{model}"))
        .header("x-goog-api-key", key)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| krabb_core::Error::Denied("Gemini model discovery failed.".into()))?;

    if !response.status().is_success() {
        return Err(krabb_core::Error::Denied(format!(
            "Gemini model discovery was rejected with {}.",
            response.status(),
        )));
    }

    metadata(&serde_json::from_slice(&super::collect_response(response).await?)?, &model)
}

fn metadata(body: &serde_json::Value, model: &str) -> krabb_core::Result<Profile> {
    let tokens = body["inputTokenLimit"].as_u64().ok_or_else(|| {
        krabb_core::Error::Protocol("Gemini model metadata omitted its input limit.".into())
    })?;
    let window = Window {
        tokens,
        output: body["outputTokenLimit"].as_u64().unwrap_or(4096).min(4096).min(tokens / 2),
    };

    window.input()?;
    Ok(Profile {
        window: Some(window),
        compact: false,
        search: model.starts_with("gemini-2.") || model.starts_with("gemini-3"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_model_limits_without_assuming_all_models_support_search() {
        let body = json!({"inputTokenLimit":1048576,"outputTokenLimit":65536});
        let profile = metadata(&body, "gemini-3.5-flash-lite").unwrap();

        assert_eq!(profile.window, Some(Window { tokens: 1048576, output: 4096 }));
        assert!(profile.search);
        assert!(!metadata(&body, "custom").unwrap().search);
        assert!(metadata(&json!({}), "model").is_err());
        assert!(metadata(&json!({"inputTokenLimit":1}), "model").is_err());
    }

    #[tokio::test]
    async fn discovery_normalizes_http_metadata_and_fails_closed() {
        for (status, body, succeeds) in [
            (200, r#"{"inputTokenLimit":8192,"outputTokenLimit":1024}"#, true),
            (403, "{}", false),
            (200, "{}", false),
            (200, "invalid", false),
        ] {
            let (base, server) = krabb_test::http::responses(vec![(status, body.into())]).await;
            let input: ModelRequest = serde_json::from_value(
                json!({"model":"gemini-3.5-flash-lite","messages":[],"tools":[],"stream":false}),
            )
            .unwrap();
            let result = profile(&reqwest::Client::new(), &input, "fixture", &base).await;

            assert_eq!(result.is_ok(), succeeds);

            if let Ok(profile) = result {
                assert_eq!(profile.window, Some(Window { tokens: 8192, output: 1024 }));
            }

            let requests = server.await.unwrap();

            assert!(requests[0].headers.starts_with("GET /models/gemini-3.5-flash-lite "));
            assert!(requests[0].headers.to_lowercase().contains("x-goog-api-key: fixture"));
        }
    }
}
