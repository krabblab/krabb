const DEFAULT: &str = "claude-haiku-5-5";

pub(super) fn resolve(model: &str) -> String {
    if matches!(model.trim(), "" | "default" | "unset") { DEFAULT.into() } else { model.into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_small_and_explicit_models_are_preserved() {
        for model in ["", " ", "default", "unset", " default "] {
            assert_eq!(resolve(model), "claude-haiku-5-5");
        }

        assert_eq!(resolve("chosen-model"), "chosen-model");
    }
}
