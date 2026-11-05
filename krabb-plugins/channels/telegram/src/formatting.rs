use krabb_text::{Document, Style};
use serde_json::{Value, json};

pub fn parts(text: &str, draft: bool) -> Vec<Value> {
    Document::parse(text, true)
        .chunks(4096, true)
        .into_iter()
        .take(if draft { 1 } else { usize::MAX })
        .map(|doc| {
            let mut entities: Vec<Value> = Vec::new();

            let mut offset = 0;

            for (text, styles) in doc.runs() {
                let length = text.encode_utf16().count();
                let linked = styles.iter().any(|style| matches!(style, Style::Link(_)));

                let code = styles
                    .iter()
                    .find(|style| !linked && matches!(style, Style::Code | Style::Pre(_)));

                let mut kinds = Vec::new();

                for style in styles.iter().filter(|style| code.is_none_or(|code| *style == code)) {
                    if linked && matches!(style, Style::Code | Style::Pre(_)) {
                        continue;
                    }

                    let mut entity = json!({"offset": offset, "length": length});

                    let kind = match style {
                        Style::Bold | Style::Heading(_) => "bold",
                        Style::Italic => "italic",
                        Style::Strike => "strikethrough",
                        Style::Code => "code",

                        Style::Pre(language) => {
                            entity["language"] = json!(language);
                            "pre"
                        }

                        Style::Quote => "blockquote",

                        Style::Link(url) => {
                            entity["url"] = json!(url);
                            "text_link"
                        }
                    };

                    if !kinds.contains(&kind) {
                        kinds.push(kind);
                        entity["type"] = json!(kind);

                        if let Some(previous) = entities.iter_mut().rev().find(|previous| {
                            previous["type"] == entity["type"]
                                && previous["url"] == entity["url"]
                                && previous["language"] == entity["language"]
                                && previous["offset"].as_u64().unwrap()
                                    + previous["length"].as_u64().unwrap()
                                    == offset as u64
                        }) {
                            previous["length"] =
                                json!(previous["length"].as_u64().unwrap() + length as u64);
                        } else {
                            entities.push(entity);
                        }
                    }
                }

                offset += length;
            }

            json!({"text": doc.text, "entities": entities})
        })
        .collect()
}

pub fn edits(
    base: &str,
    chat: i64,
    message: i64,
    text: &str,
    thread: Option<&str>,
    draft: bool,
) -> krabb_core::Result<Vec<(String, Value)>> {
    let thread = thread
        .map(|value| {
            value
                .parse::<i64>()
                .map_err(|_| krabb_core::Error::Denied("Telegram thread ID was invalid.".into()))
        })
        .transpose()?;

    Ok(parts(text, draft)
        .into_iter()
        .enumerate()
        .map(|(index, mut body)| {
            body["chat_id"] = json!(chat);

            let method = if index == 0 {
                body["message_id"] = json!(message);
                "editMessageText"
            } else {
                if let Some(thread) = thread {
                    body["message_thread_id"] = json!(thread);
                }

                "sendMessage"
            };

            (format!("{base}/{method}"), body)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_native_entities_and_counts_astral_unicode() {
        let parts =
            parts("👋 **bold *italic*** [repo](https://github.com/krabblab/krabb) `<x>&`", false);

        let body = &parts[0];

        assert_eq!(body["text"], "👋 bold italic repo <x>&");

        assert!(
            body["entities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["type"] == "bold" && e["offset"] == 3)
        );

        assert!(
            body["entities"].as_array().unwrap().iter().any(
                |e| e["type"] == "text_link" && e["url"] == "https://github.com/krabblab/krabb"
            )
        );

        assert!(body.get("parse_mode").is_none());
    }

    #[test]
    fn chunks_keep_entities_valid_and_drafts_never_append_overflow() {
        let source = format!("**{}**", "🦀".repeat(3000));
        let output = parts(&source, false);

        assert_eq!(output.len(), 2);
        assert_eq!(output[0]["entities"][0]["length"], 4096);
        assert_eq!(output[1]["entities"][0]["offset"], 0);
        assert_eq!(parts(&source, true).len(), 1);

        let edits = edits("https://example.com", 7, 9, &source, Some("4"), false).unwrap();

        assert!(edits[0].0.ends_with("editMessageText"));
        assert_eq!(edits[1].1["message_thread_id"], 4);
        assert!(super::edits("", 7, 9, "x", Some("bad"), false).is_err());
    }
}
