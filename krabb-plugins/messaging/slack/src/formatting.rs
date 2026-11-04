use krabb_text::{Document, Style};
use serde_json::{Value, json};

pub fn bodies(channel: &str, source: &str, thread: &Value) -> Vec<Value> {
    Document::parse(source, true)
        .chunks(3000, false)
        .into_iter()
        .map(|doc| {
            let mut sections: Vec<Value> = Vec::new();

            for (text, styles) in doc.runs() {
                let mut style = json!({});
                let mut element = json!({"type": "text", "text": text});
                let mut kind = "rich_text_section";

                for mark in styles {
                    match mark {
                        Style::Bold | Style::Heading(_) => style["bold"] = json!(true),
                        Style::Italic => style["italic"] = json!(true),
                        Style::Strike => style["strike"] = json!(true),
                        Style::Code => style["code"] = json!(true),
                        Style::Pre(_) => kind = "rich_text_preformatted",

                        Style::Quote if kind != "rich_text_preformatted" => {
                            kind = "rich_text_quote"
                        }

                        Style::Link(url) => {
                            element["type"] = json!("link");
                            element["url"] = json!(url);
                        }

                        _ => {}
                    }
                }

                if !style.as_object().unwrap().is_empty() && kind != "rich_text_preformatted" {
                    element["style"] = style;
                }

                if sections.last().is_none_or(|section| section["type"] != kind) {
                    sections.push(json!({"type": kind, "elements": []}));
                }

                sections.last_mut().unwrap()["elements"].as_array_mut().unwrap().push(element);
            }

            json!({"channel": channel, "thread_ts": thread, "text": doc.text, "mrkdwn": false,
            "blocks": [{"type": "rich_text", "elements": sections}]})
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_native_elements_without_interpreting_literal_mentions_or_markup() {
        let output = bodies(
            "C1",
            "**bold** [repo](https://example.com) `<@U1>&`\n\n> quote\n\n```rs\nx < y\n```",
            &json!("thread"),
        );

        let encoded = output[0].to_string();

        assert!(encoded.contains("rich_text_quote"));
        assert!(encoded.contains("rich_text_preformatted"));
        assert!(encoded.contains("\"bold\":true"));
        assert!(encoded.contains("\"type\":\"link\""));
        assert_eq!(output[0]["mrkdwn"], false);
        assert_eq!(output[0]["thread_ts"], "thread");
        assert!(output[0]["text"].as_str().unwrap().contains("<@U1>&"));
        assert_eq!(bodies("C", &"🦀".repeat(6001), &Value::Null).len(), 3);
    }
}
