const DEFAULT: &str = "gpt-6-luna";

pub(super) fn resolve(model: &str) -> String {
    if matches!(model.trim(), "" | "default" | "unset") { DEFAULT.into() } else { model.into() }
}

pub(super) fn configure(body: &mut serde_json::Value) {
    if body["model"] == DEFAULT {
        // Luna's Chat Completions tool calling requires reasoning to be disabled.
        body["reasoning_effort"] = "none".into();
        body["max_completion_tokens"] = 4096.into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_small_and_explicit_models_are_preserved() {
        for model in ["", " ", "default", "unset", " default "] {
            assert_eq!(resolve(model), "gpt-6-luna");
        }

        assert_eq!(resolve("chosen-model"), "chosen-model");
    }

    #[test]
    fn luna_supports_tools_with_bounded_output_and_no_reasoning() {
        let mut body = serde_json::json!({"model": DEFAULT, "tools": [{"type":"function"}]});
        configure(&mut body);

        assert_eq!(body["reasoning_effort"], "none");
        assert_eq!(body["max_completion_tokens"], 4096);
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);

        let mut custom = serde_json::json!({"model": "custom"});
        configure(&mut custom);

        assert_eq!(custom, serde_json::json!({"model": "custom"}));
    }
}
