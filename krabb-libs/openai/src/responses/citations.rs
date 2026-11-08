use serde_json::Value;

/// Place source links at the provider's character offsets, preserving original text.
pub(super) fn text(part: &Value) -> String {
    let text = part["text"].as_str().or_else(|| part["refusal"].as_str()).unwrap_or_default();
    let mut links = Vec::new();

    for annotation in part["annotations"].as_array().into_iter().flatten().take(32) {
        if let Some(link) = krabb_text::citation(
            annotation["url"].as_str().unwrap_or_default(),
            annotation["title"].as_str().unwrap_or_default(),
        ) {
            let offset = annotation["end_index"]
                .as_u64()
                .and_then(|index| usize::try_from(index).ok())
                .and_then(|index| text.char_indices().nth(index).map(|(offset, _)| offset))
                .unwrap_or(text.len());

            if !links.contains(&(offset, link.clone())) {
                links.push((offset, link));
            }
        }
    }

    links.sort_by_key(|(offset, _)| *offset);
    let mut output = String::new();
    let mut previous = 0;

    for (offset, link) in links {
        output.push_str(&text[previous..offset]);
        output.push(' ');
        output.push_str(&link);
        previous = offset;
    }

    output.push_str(&text[previous..]);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn source_offsets_preserve_unicode_and_fall_back_to_the_containing_part() {
        let value = json!({"text":"🦀 One. Two.", "annotations":[
            {"end_index":6,"url":"https://one.example","title":"One"},
            {"end_index":6,"url":"https://one.example","title":"One"},
            {"end_index":999,"url":"https://two.example","title":"Two"},
            {"url":"javascript:alert(1)","title":"unsafe"}
        ]});

        assert_eq!(
            text(&value),
            "🦀 One. [One](https://one.example) Two. [Two](https://two.example)"
        );

        assert_eq!(text(&json!({"refusal":"refused"})), "refused");
    }
}
