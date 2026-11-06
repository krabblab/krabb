use std::ops::Range;

use krabb_text::{Document, Style as TextStyle};

use ratatui::{
    style::{Color, Modifier, Style},
    text::Line,
};

pub(super) fn render(source: &str, width: usize) -> Vec<Line<'static>> {
    if source.len() > krabb_text::LIMIT {
        return source
            .lines()
            .flat_map(|line| {
                super::wrap_styled_segments(&[(visible(line), Style::default())], width)
            })
            .collect();
    }

    let document = Document::parse(source, false);
    let runs = styles(&document);
    let mut blocks = document.spans.iter().filter_map(|span| match &span.style {
        TextStyle::Pre(language) => Some((span.start..span.end, language)),
        _ => None,
    });

    let mut block = blocks.next();
    let mut offset = 0;
    let mut rows = Vec::new();

    while offset < document.text.len() {
        if let Some((range, language)) = &block
            && range.start == offset
        {
            let code = &document.text[range.clone()];
            let code = code.strip_suffix('\n').unwrap_or(code);
            let lines = code.split('\n').map(visible).collect::<Vec<_>>();
            let lines = lines.iter().map(String::as_str).collect::<Vec<_>>();
            let language = if language.is_empty() { "code" } else { language };

            rows.extend(super::wrap_code_block(&lines, language, width));
            offset = range.end;
            block = blocks.next();
            continue;
        }

        let line_end =
            document.text[offset..].find('\n').map_or(document.text.len(), |n| offset + n);
        let end = block.as_ref().map_or(line_end, |(range, _)| line_end.min(range.start));
        let first = runs.partition_point(|(range, _)| range.end <= offset);
        let segments = runs[first..]
            .iter()
            .take_while(|(range, _)| range.start < end)
            .map(|(range, style)| {
                (visible(&document.text[range.start.max(offset)..range.end.min(end)]), *style)
            })
            .collect::<Vec<_>>();

        rows.extend(super::wrap_styled_segments(&segments, width));

        let starts_block = block.as_ref().is_some_and(|(range, _)| range.start == end);
        offset = end.saturating_add(usize::from(!starts_block));
    }

    rows
}

fn styles(document: &Document) -> Vec<(Range<usize>, Style)> {
    let mut events = Vec::new();

    for span in &document.spans {
        let kind = match span.style {
            TextStyle::Bold | TextStyle::Heading(_) => 0,
            TextStyle::Italic => 1,
            TextStyle::Strike => 2,
            TextStyle::Code => 3,
            TextStyle::Quote => 4,
            _ => continue,
        };

        events.push((span.start, kind, true));
        events.push((span.end, kind, false));
    }

    events.sort_unstable();

    let mut active = [0_usize; 5];
    let mut offset = 0;
    let mut runs = Vec::new();

    for (position, kind, start) in events {
        if position > offset {
            runs.push((offset..position, style(active)));
            offset = position;
        }

        if start {
            active[kind] += 1;
        } else {
            active[kind] -= 1;
        }
    }

    if offset < document.text.len() {
        runs.push((offset..document.text.len(), style(active)));
    }

    runs
}

fn style(active: [usize; 5]) -> Style {
    let mut style = Style::default();

    for (count, modifier) in active.into_iter().zip([
        Modifier::BOLD,
        Modifier::ITALIC,
        Modifier::CROSSED_OUT,
        Modifier::DIM,
        Modifier::ITALIC,
    ]) {
        if count > 0 {
            style = style.add_modifier(modifier);
        }
    }

    if active[3] > 0 {
        style = style.fg(Color::Yellow);
    }

    style
}

pub(super) fn visible(text: &str) -> String {
    let mut visible = String::new();

    for character in text.chars() {
        match character {
            '\t' => visible.push_str("    "),
            character if character.is_control() => visible.push('�'),
            character => visible.push(character),
        }
    }

    visible
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(rows: &[Line<'_>]) -> String {
        rows.iter()
            .map(|row| row.spans.iter().map(|span| span.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_nested_emphasis_code_headings_quotes_and_links() {
        let rows = render(
            "# Today\n\n**bold *both*** and _italic_ and ~~old~~ and `a_b * c`.\n\n\
             > quoted\n\n[ANSA](https://www.ansa.it/english/news/)",
            100,
        );

        assert_eq!(
            text(&rows),
            "Today\n\nbold both and italic and old and a_b * c.\n\n\
             quoted\n\nANSA (https://www.ansa.it/english/news/)"
        );

        let spans = rows.iter().flat_map(|row| &row.spans).collect::<Vec<_>>();
        let styled = |word: &str, modifiers| {
            spans.iter().any(|span| {
                span.content.contains(word) && span.style.add_modifier.contains(modifiers)
            })
        };

        assert!(styled("Today", Modifier::BOLD));
        assert!(styled("bold", Modifier::BOLD));
        assert!(styled("both", Modifier::BOLD | Modifier::ITALIC));
        assert!(styled("italic", Modifier::ITALIC));
        assert!(styled("old", Modifier::CROSSED_OUT));
        assert!(styled("a_b * c", Modifier::DIM));
        assert!(styled("quoted", Modifier::ITALIC));
        assert!(
            spans
                .iter()
                .any(|span| { span.content.contains(" and ") && span.style == Style::default() })
        );
    }

    #[test]
    fn wraps_unicode_styles_and_keeps_unmatched_or_escaped_markers_literal() {
        let rows = render("**こんにちは e\u{301} 👩‍💻 world** plain", 10);

        assert!(rows.iter().all(|row| row.width() <= 10));
        assert!(rows.iter().flat_map(|row| &row.spans).any(|span| {
            span.content.contains("👩‍💻") && span.style.add_modifier.contains(Modifier::BOLD)
        }));

        assert_eq!(
            text(&render(r"\*literal\* foo_bar **unfinished", 80)),
            "*literal* foo_bar **unfinished"
        );

        assert_eq!(text(&render("**first\nsecond**", 80)), "first\nsecond");
        assert!(render("**first\nsecond**", 80).iter().all(|row| {
            row.spans.iter().all(|span| span.style.add_modifier.contains(Modifier::BOLD))
        }));
    }

    #[test]
    fn preserves_indented_code_and_neutralizes_control_characters() {
        let rows = render("Example:\n\n    **literal**\n    \tcode\n\nEnd.", 40);

        assert!(text(&rows).contains("┌─ code"));
        assert!(text(&rows).contains("**literal**"));
        assert!(text(&rows).contains("    code"));
        assert!(text(&rows).ends_with("End."));
        assert_eq!(text(&render("a\u{1b}[31m\u{7}\tb", 80)), "a�[31m�    b");

        assert!(render("", 80).is_empty());
    }

    #[test]
    fn renders_code_nested_in_a_list_before_later_blocks() {
        let rows = render("-     first\n\n      second\n\nAfter\n\n    third", 40);
        let rendered = text(&rows);

        assert_eq!(rendered.matches("┌─ code").count(), 2);
        assert!(rendered.contains("first"));
        assert!(rendered.contains("second"));
        assert!(rendered.contains("After"));
        assert!(rendered.contains("third"));

        let rows = render("- ```rust\n\n  **literal**\n  ```", 40);
        let rendered = text(&rows);

        assert!(rendered.contains("┌─ rust"));
        assert!(rendered.contains("**literal**"));
    }

    #[test]
    fn oversized_text_is_preserved_with_plain_fallback() {
        let source = format!("{}\nTHE END", "x".repeat(krabb_text::LIMIT + 1));
        let rows = render(&source, 80);

        assert_eq!(text(&rows).replace('\n', ""), source.replace('\n', ""));
        assert!(rows.iter().all(|row| row.width() <= 80));
    }

    #[test]
    fn every_streamed_prefix_remains_renderable() {
        let source = "**bold _nested_** `code` [link](https://example.com)\n\n> 界";

        for end in source.char_indices().map(|(offset, _)| offset).chain([source.len()]) {
            let rows = render(&source[..end], 12);

            assert!(rows.iter().all(|row| row.width() <= 12));
        }
    }
}
