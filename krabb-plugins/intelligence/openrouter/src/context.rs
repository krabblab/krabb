use krabb_core::context::{Profile, Window};
use serde_json::Value;

pub(super) async fn profile(
    client: &reqwest::Client,
    model: &str,
    key: &str,
    base: &str,
) -> krabb_core::Result<Profile> {
    let (author, slug) = model.split_once('/').ok_or_else(|| {
        krabb_core::Error::Protocol(
            "OpenRouter model must include an author and model name.".into(),
        )
    })?;
    let mut url = reqwest::Url::parse(&format!("{base}/model/"))
        .map_err(|_| krabb_core::Error::Denied("OpenRouter model URL is invalid.".into()))?;
    url.path_segments_mut().unwrap().pop_if_empty().push(author).push(slug);
    let response = client
        .get(url)
        .bearer_auth(key)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| krabb_core::Error::Denied("OpenRouter model discovery failed.".into()))?;

    if !response.status().is_success() {
        return Err(krabb_core::Error::Denied(format!(
            "OpenRouter model discovery was rejected with {}.",
            response.status(),
        )));
    }

    let body: Value = serde_json::from_slice(&super::collect(response.bytes_stream()).await?)?;
    let mut profile = metadata(&body["data"])?;
    let endpoints = super::search::endpoints(client, model, key, base).await?;

    if let Some(window) = profile.window.as_mut() {
        for endpoint in &endpoints {
            window.tokens = window.tokens.min(endpoint.window.tokens);
            window.output = window.output.min(endpoint.window.output);
        }

        window.output = window.output.min(window.tokens / 2);
    }

    profile.search = !endpoints.is_empty();
    Ok(profile)
}

fn metadata(body: &Value) -> krabb_core::Result<Profile> {
    let tokens = body["context_length"].as_u64().ok_or_else(|| {
        krabb_core::Error::Protocol("OpenRouter model metadata omitted its context limit.".into())
    })?;
    let tokens = tokens.min(body["top_provider"]["context_length"].as_u64().unwrap_or(tokens));
    let output = body["top_provider"]["max_completion_tokens"].as_u64().unwrap_or(4096);
    let window = Window { tokens, output: output.min(4096).min(tokens / 2) };

    window.input()?;
    Ok(Profile { window: Some(window), compact: false, search: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn routed_limits_do_not_exceed_the_reported_provider_window() {
        let profile = metadata(&json!({"context_length":131072,
            "top_provider":{"context_length":32768,"max_completion_tokens":2048}}))
        .unwrap();

        assert_eq!(profile.window, Some(Window { tokens: 32768, output: 2048 }));
        assert_eq!(metadata(&json!({"context_length":4096})).unwrap().window.unwrap().output, 2048);
        assert!(metadata(&json!({})).is_err());
        assert!(metadata(&json!({"context_length":0})).is_err());
    }
}
