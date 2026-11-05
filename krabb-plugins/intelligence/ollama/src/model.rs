const LOCAL: &str = "qwen3.5:4b";
const CLOUD: &str = "deepseek-v4.1-flash";

pub(super) fn resolve(model: &str, host: &str) -> String {
    if !matches!(model.trim(), "" | "default" | "unset") {
        return model.into();
    }

    let cloud = reqwest::Url::parse(host)
        .is_ok_and(|url| url.scheme() == "https" && url.host_str() == Some("ollama.com"));

    if cloud { CLOUD.into() } else { LOCAL.into() }
}

pub(super) fn configure(body: &mut serde_json::Value) {
    if body["model"] == LOCAL || body["model"] == CLOUD {
        body["think"] = false.into();
        body["options"] = serde_json::json!({"num_predict": 4096});
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_current_local_and_cloud_models_without_changing_explicit_choices() {
        for model in ["", " ", "default", "unset", " default "] {
            assert_eq!(resolve(model, "http://127.0.0.1:11434"), "qwen3.5:4b");
            assert_eq!(resolve(model, "https://ollama.com/"), "deepseek-v4.1-flash");
        }

        for host in ["https://ollama.com", "http://localhost:11434"] {
            assert_eq!(resolve("custom:cloud", host), "custom:cloud");
        }

        for host in
            ["http://ollama.com", "https://ollama.com.example", "https://proxy.example", "invalid"]
        {
            assert_eq!(resolve("default", host), LOCAL);
        }
    }

    #[test]
    fn bounds_default_generation_and_preserves_images() {
        for model in [LOCAL, CLOUD] {
            let mut body = serde_json::json!({"model":model,"messages":[]});
            configure(&mut body);

            assert_eq!(body["think"], false);
            assert_eq!(body["options"]["num_predict"], 4096);
        }

        let mut image = serde_json::json!({"model":LOCAL,"messages":[{"images":["image"]}]});

        configure(&mut image);

        assert_eq!(image["messages"][0]["images"][0], "image");

        image["model"] = CLOUD.into();
        configure(&mut image);

        let mut explicit = serde_json::json!({"model":"custom","messages":[]});
        configure(&mut explicit);

        assert!(explicit.get("think").is_none());
        assert!(explicit.get("options").is_none());
    }
}
