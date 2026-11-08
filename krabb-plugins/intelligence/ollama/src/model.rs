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

pub(super) fn configure(
    body: &mut serde_json::Value,
    thinking: &str,
    context: &str,
) -> krabb_core::Result<()> {
    let thinking = match thinking.trim() {
        "auto" => None,
        "true" => Some(true.into()),
        "" | "false" => Some(false.into()),
        level @ ("low" | "medium" | "high") => Some(serde_json::Value::from(level)),

        _ => {
            return Err(krabb_core::Error::Denied(
                "KRABB_OLLAMA_THINK must be auto, true, false, low, medium, or high.".into(),
            ));
        }
    };

    let context = match context.trim() {
        "" | "auto" => None,
        value => Some(value.parse::<u32>().ok().filter(|value| *value > 0).ok_or_else(|| {
            krabb_core::Error::Denied(
                "KRABB_OLLAMA_CONTEXT_LENGTH must be auto or a positive 32-bit token count.".into(),
            )
        })?),
    };

    if body["model"] == LOCAL || body["model"] == CLOUD {
        body["options"] = serde_json::json!({"num_predict": 4096});
    }

    if let Some(thinking) = thinking {
        body["think"] = thinking;
    }

    if let Some(context) = context {
        body["options"]["num_ctx"] = context.into();
    }

    Ok(())
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
            configure(&mut body, "", "auto").unwrap();

            assert_eq!(body["think"], false);
            assert_eq!(body["options"]["num_predict"], 4096);
        }

        let mut image = serde_json::json!({"model":LOCAL,"messages":[{"images":["image"]}]});

        configure(&mut image, "", "").unwrap();

        assert_eq!(image["messages"][0]["images"][0], "image");

        image["model"] = CLOUD.into();
        configure(&mut image, "auto", "auto").unwrap();

        let mut explicit = serde_json::json!({"model":"custom","messages":[]});
        configure(&mut explicit, "auto", "auto").unwrap();

        assert!(explicit.get("think").is_none());
        assert!(explicit.get("options").is_none());

        configure(&mut explicit, "", "").unwrap();

        assert_eq!(explicit["think"], false);
    }

    #[test]
    fn settings_override_defaults_and_support_explicit_models() {
        for model in [LOCAL, CLOUD, "qwen3.5:0.8b"] {
            for (setting, expected) in [
                ("false", serde_json::json!(false)),
                ("true", serde_json::json!(true)),
                ("low", serde_json::json!("low")),
                ("medium", serde_json::json!("medium")),
                ("high", serde_json::json!("high")),
            ] {
                let mut body = serde_json::json!({"model":model,"messages":[]});
                configure(&mut body, setting, " 8192 ").unwrap();

                assert_eq!(body["think"], expected);
                assert_eq!(body["options"]["num_ctx"], 8192);

                if model != "qwen3.5:0.8b" {
                    assert_eq!(body["options"]["num_predict"], 4096);
                } else {
                    assert!(body["options"].get("num_predict").is_none());
                }
            }
        }
    }

    #[test]
    fn malformed_settings_fail_without_echoing_values() {
        for (thinking, context) in [
            ("private-input", "auto"),
            ("auto", "private-input"),
            ("auto", "0"),
            ("auto", "-1"),
            ("auto", "1.5"),
            ("auto", "4294967296"),
        ] {
            let mut body = serde_json::json!({"model":LOCAL});
            let error = configure(&mut body, thinking, context).unwrap_err().to_string();

            assert!(error.contains("KRABB_OLLAMA_"));
            assert!(!error.contains("private-input"));
            assert_eq!(body, serde_json::json!({"model":LOCAL}));
        }
    }
}
