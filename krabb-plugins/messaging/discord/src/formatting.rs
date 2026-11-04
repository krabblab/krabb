use krabb_text::{Document, Syntax};
use serde_json::{Value, json};

pub fn parts(source: &str, draft: bool) -> Vec<Value> {
    let syntax = Syntax {
        bold: "**",
        italic: "*",
        strike: "~~",
        escape: true,
        links: true,
        headings: true,
        language: true,
    };

    syntax
        .chunks(&Document::parse(source, true), 2000)
        .into_iter()
        .take(if draft { 1 } else { usize::MAX })
        .map(|text| json!({"content": text, "allowed_mentions": {"parse": []}}))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_markup_and_bounds_complete_chunks() {
        let source = format!("**{}**\n\n[repo](https://example.com)", "🦀".repeat(2100));
        let parts = parts(&source, false);

        assert!(parts.len() > 1);
        assert!(parts.iter().all(|part| part["content"].as_str().unwrap().chars().count() <= 2000));
        assert!(parts[0]["content"].as_str().unwrap().starts_with("**"));
        assert!(parts[0]["content"].as_str().unwrap().ends_with("**"));
        assert_eq!(parts[0]["allowed_mentions"]["parse"], json!([]));
        assert_eq!(super::parts(&source, true).len(), 1);
    }
}
