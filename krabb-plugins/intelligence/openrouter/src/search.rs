use super::*;
use krabb_core::context::Window;

pub(super) struct Endpoint {
    pub tag: String,
    pub window: Window,
}

pub(super) async fn endpoints(
    client: &reqwest::Client,
    model: &str,
    key: &str,
    base: &str,
) -> krabb_core::Result<Vec<Endpoint>> {
    let Some((author, slug)) = model.split_once('/') else { return Ok(vec![]) };

    let mut url = reqwest::Url::parse(&format!("{base}/models/"))
        .map_err(|_| krabb_core::Error::Denied("OpenRouter model URL is invalid.".into()))?;

    url.path_segments_mut().unwrap().pop_if_empty().push(author).push(slug).push("endpoints");

    let response = client
        .get(url)
        .bearer_auth(key)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| krabb_core::Error::Denied("OpenRouter search discovery failed.".into()))?;

    if matches!(response.status().as_u16(), 403 | 404) {
        return Ok(vec![]);
    }

    if !response.status().is_success() {
        return Err(krabb_core::Error::Denied(format!(
            "OpenRouter search discovery was rejected with {}.",
            response.status()
        )));
    }

    let value: Value = serde_json::from_slice(&collect(response.bytes_stream()).await?)?;
    let mut native = Vec::new();

    for endpoint in value["data"]["endpoints"].as_array().into_iter().flatten() {
        if endpoint["native_tools"]["openrouter:web_search"].is_null() {
            continue;
        }

        let Some(tag) = endpoint["tag"].as_str().filter(|tag| !tag.is_empty() && tag.len() <= 256)
        else {
            continue;
        };

        let Some(tokens) = endpoint["context_length"].as_u64() else { continue };

        let window = Window {
            tokens,
            output: endpoint["max_completion_tokens"]
                .as_u64()
                .unwrap_or(4096)
                .min(4096)
                .min(tokens / 2),
        };

        if window.input().is_ok() && native.len() < 16 {
            native.push(Endpoint { tag: tag.into(), window });
        }
    }

    Ok(native)
}

pub(super) async fn configure(
    client: &reqwest::Client,
    input: &ModelRequest,
    body: &mut Value,
    key: &str,
    base: &str,
) -> krabb_core::Result<()> {
    body["plugins"] = json!([{"id":"web", "enabled":false}]);

    if !input.generation.is_some_and(|value| value.search && !value.summary) {
        return Ok(());
    }

    let native = endpoints(client, body["model"].as_str().unwrap_or_default(), key, base).await?;

    if native.is_empty() {
        return Ok(());
    }

    // `engine: native` alone permits external fallback. Restrict routing to advertised
    // native endpoints as well, and never enable OpenRouter's account-level web plugin.
    body["provider"] = json!({
        "only":native.iter().map(|endpoint| &endpoint.tag).collect::<Vec<_>>(),
        "allow_fallbacks":false, "require_parameters":true,
    });

    body["tools"].as_array_mut().unwrap().push(json!({
        "type":"openrouter:web_search", "parameters":{"engine":"native", "max_uses":3},
    }));

    Ok(())
}

pub(super) fn citations(value: &Value) -> Vec<String> {
    value["annotations"]
        .as_array()
        .into_iter()
        .flatten()
        .take(32)
        .filter_map(|annotation| {
            let citation = &annotation["url_citation"];
            krabb_text::citation(
                citation["url"].as_str()?,
                citation["title"].as_str().unwrap_or_default(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests;
