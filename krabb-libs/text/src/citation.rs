/// Render a provider citation as a bounded CommonMark link understood by every channel.
pub fn citation(url: &str, title: &str) -> Option<String> {
    if url.len() > 8192
        || !(url.starts_with("https://") || url.starts_with("http://"))
        || !super::safe_link(url)
    {
        return None;
    }

    let title = if title.trim().is_empty() { "Source" } else { title.trim() };

    let mut label = String::new();

    for ch in title.chars().take(256) {
        if "\\*_~`[]()<>#|&!".contains(ch) {
            label.push('\\');
        }

        let character = if ch.is_control() { ' ' } else { ch };

        label.push(character);
    }

    let target = url
        .replace('(', "%28")
        .replace(')', "%29")
        .replace('<', "%3C")
        .replace('>', "%3E")
        .replace('\\', "%5C")
        .replace(' ', "%20");

    Some(format!("[{label}]({target})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citations_preserve_attribution_without_injecting_markup() {
        let link = citation("https://example.com/a(b)", "[A] *B* &copy;\nnext").unwrap();
        let doc = crate::Document::parse(&link, true);

        assert_eq!(doc.text, "[A] *B* &copy; next");
        assert_eq!(doc.spans.len(), 1);
        assert_eq!(doc.spans[0].style, crate::Style::Link("https://example.com/a%28b%29".into()));
        assert_eq!(citation("http://example.com", " ").unwrap(), "[Source](http://example.com)");
        assert!(citation("javascript:alert(1)", "bad").is_none());
        assert!(citation("https://example.com/\n", "bad").is_none());
        assert!(citation(&format!("https://example.com/{}", "x".repeat(8192)), "bad").is_none());
        assert!(citation("https://example.com", &"x".repeat(1000)).unwrap().len() < 300);
    }
}
