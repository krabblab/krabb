//! OpenAI-specific integration shared by the API-key and Codex providers.
#![forbid(unsafe_code)]

pub mod responses;

use krabb_core::context::{Profile, Window};

/// The Models API does not publish context limits; use documented model families
/// with an explicit override for deployments and models outside this catalog.
pub fn profile(model: &str, configured: &str) -> krabb_core::Result<Profile> {
    let tokens = match configured.trim() {
        "" | "auto" => capacity(model),
        value => value.parse::<u32>().ok().filter(|value| *value > 1).map(u64::from).ok_or_else(
            || {
                krabb_core::Error::Denied(
                    "Context length must be auto or a positive token count greater than one."
                        .into(),
                )
            },
        )?,
    };

    let search = search(model);

    // The hosted search tool has a smaller context limit than long-context models.
    let tokens = if search { tokens.min(128000) } else { tokens };

    let window = Window { tokens, output: 4096.min(tokens / 2) };

    window.input()?;
    Ok(Profile { window: Some(window), compact: false, search })
}

fn capacity(model: &str) -> u64 {
    if model.starts_with("gpt-6") {
        1_050_000
    } else if model.starts_with("gpt-5") {
        400_000
    } else if model.starts_with("gpt-4.1") {
        1_047_576
    } else if model.starts_with("gpt-4o") {
        128_000
    } else if model.starts_with("o3") || model.starts_with("o4") {
        200_000
    } else {
        4096
    }
}

pub fn search(model: &str) -> bool {
    (model.starts_with("gpt-6")
        || model.starts_with("gpt-5")
        || model.starts_with("gpt-4.1")
        || model.starts_with("gpt-4o")
        || model == "o3"
        || model.starts_with("o4-mini"))
        && !model.starts_with("gpt-4.1-nano")
        && !["audio", "realtime", "search", "transcribe", "tts"]
            .iter()
            .any(|kind| model.contains(kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_families_reserve_output_and_observe_the_search_limit() {
        for (model, capacity) in [
            ("gpt-6-luna", 1050000),
            ("gpt-5", 400000),
            ("gpt-4.1", 1047576),
            ("gpt-4o", 128000),
            ("o3", 200000),
            ("o4-mini", 200000),
        ] {
            assert_eq!(super::capacity(model), capacity);
            let profile = profile(model, "auto").unwrap();

            assert!(profile.search);
            assert_eq!(profile.window, Some(Window { tokens: 128000, output: 4096 }));
        }

        assert!(!search("gpt-4.1-nano"));

        assert!(!search("custom"));
        assert_eq!(profile("custom", "").unwrap().window.unwrap().tokens, 4096);
        assert_eq!(profile("custom", "32768").unwrap().window.unwrap().tokens, 32768);

        for invalid in ["private-value", "0", "1", "-1", "4294967296"] {
            let error = profile("custom", invalid).unwrap_err().to_string();

            assert!(!error.contains("private-value"));
        }
    }
}
