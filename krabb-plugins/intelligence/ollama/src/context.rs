use krabb_core::context::{Profile, Window};

pub(super) async fn profile(
    client: &reqwest::Client,
    model: &str,
    host: &str,
    configured: &str,
    key: Option<&str>,
) -> krabb_core::Result<Profile> {
    let mut settings = serde_json::json!({});

    super::model::configure(&mut settings, "auto", configured)?;

    let tokens = if let Some(tokens) = settings["options"]["num_ctx"].as_u64() {
        tokens
    } else {
        let host = super::validate_host(host, key.is_some())?;
        let mut request = client.get(format!("{host}/api/ps"));

        if let Some(key) = key {
            request = request.bearer_auth(key);
        }

        let response =
            request.timeout(std::time::Duration::from_secs(10)).send().await.map_err(|_| {
                krabb_core::Error::Denied("Ollama context discovery failed.".into())
            })?;

        // Cloud and proxy endpoints may not expose the local runner's allocation.
        let allocated = if response.status() == reqwest::StatusCode::NOT_FOUND {
            None
        } else if !response.status().is_success() {
            return Err(krabb_core::Error::Denied(format!(
                "Ollama context discovery was rejected with {}.",
                response.status(),
            )));
        } else {
            let body: serde_json::Value =
                serde_json::from_str(&super::read(response, "Ollama").await?)?;
            running(&body, model)
        };

        match allocated {
            Some(tokens) => tokens,
            None => shown(client, &host, model, key).await.unwrap_or(4096),
        }
    };

    let window = Window { tokens, output: (tokens / 2).min(4096) };

    window.input()?;
    let execution = super::execution::profile(
        host,
        model,
        &std::env::var("KRABB_OLLAMA_DEPLOYMENT").unwrap_or_default(),
        &std::env::var("KRABB_OLLAMA_REVISION").unwrap_or_default(),
        &std::env::var("KRABB_OLLAMA_THINK").unwrap_or_default(),
        configured,
    )?;
    Ok(Profile { execution: Some(execution), window: Some(window), compact: false, search: false })
}

/// The context length the model's own details report, for a model that is not running yet.
async fn shown(
    client: &reqwest::Client,
    host: &str,
    model: &str,
    key: Option<&str>,
) -> Option<u64> {
    let mut request =
        client.post(format!("{host}/api/show")).json(&serde_json::json!({"model": model}));

    if let Some(key) = key {
        request = request.bearer_auth(key);
    }

    let response = request.timeout(std::time::Duration::from_secs(10)).send().await.ok()?;

    if !response.status().is_success() {
        return None;
    }

    let body: serde_json::Value =
        serde_json::from_str(&super::read(response, "Ollama").await.ok()?).ok()?;

    body["model_info"]
        .as_object()?
        .iter()
        .find(|(name, _)| name.ends_with(".context_length"))
        .and_then(|(_, tokens)| tokens.as_u64())
}

fn running(body: &serde_json::Value, model: &str) -> Option<u64> {
    body["models"].as_array()?.iter().find(|entry| {
        entry["name"]
            .as_str()
            .or_else(|| entry["model"].as_str())
            .is_some_and(|name| name == model || name.strip_suffix(":latest") == Some(model))
    })?["context_length"]
        .as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn uses_the_running_models_allocation_instead_of_its_architecture_maximum() {
        let body = json!({"models":[
            {"name":"other","context_length":131072},
            {"name":"small:latest","context_length":4096}
        ]});

        assert_eq!(running(&body, "small"), Some(4096));
        assert_eq!(running(&body, "small:latest"), Some(4096));
        assert_eq!(running(&body, "missing"), None);
        assert_eq!(running(&json!({}), "small"), None);
    }

    #[tokio::test]
    async fn models_that_are_not_running_report_their_own_context_length() {
        let shown =
            r#"{"model_info":{"general.architecture":"gptoss","gptoss.context_length":131072}}"#;
        let responses = vec![(200, r#"{"models":[]}"#.into()), (200, shown.into())];
        let (base, server) = krabb_test::http::responses(responses).await;

        let result =
            profile(&reqwest::Client::new(), "big", &base, "auto", Some("fixture")).await.unwrap();

        assert_eq!(result.window.map(|window| window.tokens), Some(131072));

        let requests = server.await.unwrap();

        assert!(requests[1].headers.starts_with("POST /api/show "));
        assert!(requests[1].headers.to_lowercase().contains("authorization: bearer fixture"));
    }

    #[tokio::test]
    async fn explicit_context_needs_no_network_and_reserves_generation_space() {
        let profile =
            profile(&reqwest::Client::new(), "small", "unused", "4096", None).await.unwrap();

        assert_eq!(profile.window, Some(Window { tokens: 4096, output: 2048 }));
        assert!(!profile.search);
        assert!(
            self::profile(&reqwest::Client::new(), "small", "unused", "1", None).await.is_err()
        );
    }

    #[tokio::test]
    async fn discovers_allocations_and_uses_a_small_fallback_for_unloaded_models() {
        for (status, body, expected) in [
            (200, r#"{"models":[{"name":"small","context_length":8192}]}"#, Some(8192)),
            (200, r#"{"models":[]}"#, Some(4096)),
            (404, "{}", Some(4096)),
            (403, "{}", None),
            (200, "invalid", None),
        ] {
            let (base, server) = krabb_test::http::responses(vec![(status, body.into())]).await;
            let result =
                profile(&reqwest::Client::new(), "small", &base, "auto", Some("fixture")).await;
            assert_eq!(
                result.ok().and_then(|profile| profile.window).map(|window| window.tokens),
                expected
            );
            let requests = server.await.unwrap();

            assert!(requests[0].headers.starts_with("GET /api/ps "));
            assert!(requests[0].headers.to_lowercase().contains("authorization: bearer fixture"));
        }
    }
}
