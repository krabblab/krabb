#![forbid(unsafe_code)]

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd as End};

pub const LIMIT: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Style {
    Bold,
    Italic,
    Strike,
    Code,
    Pre(String),
    Heading(u8),
    Quote,
    Link(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    pub text: String,
    pub spans: Vec<Span>,
}

impl Document {
    /// Parse CommonMark with GFM tables, tasks, and strikethrough. Offsets are UTF-8 bytes.
    /// When links cannot be represented natively, retain their targets in visible text.
    pub fn parse(source: &str, links: bool) -> Self {
        let source = &source[..boundary(source, LIMIT)];
        let mut doc = Self::default();
        let mut open = Vec::new();
        let mut lists = Vec::new();

        let options =
            Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;

        for event in Parser::new_ext(source, options) {
            match event {
                Event::Start(tag) => {
                    let style = match &tag {
                        Tag::Strong => Some(Style::Bold),
                        Tag::Emphasis => Some(Style::Italic),
                        Tag::Strikethrough => Some(Style::Strike),
                        Tag::Heading { level, .. } => Some(Style::Heading(*level as u8)),
                        Tag::BlockQuote(_) => Some(Style::Quote),
                        Tag::CodeBlock(kind) => Some(Style::Pre(match kind {
                            CodeBlockKind::Fenced(language) => language
                                .split_whitespace()
                                .next()
                                .unwrap_or_default()
                                .chars()
                                .filter(|c| {
                                    c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-')
                                })
                                .take(32)
                                .collect(),
                            CodeBlockKind::Indented => String::new(),
                        })),

                        Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                            Some(Style::Link(dest_url.to_string()))
                        }

                        _ => None,
                    };

                    if let Some(style) = style {
                        open.push((tag.to_end(), doc.text.len(), style));
                    }

                    match tag {
                        Tag::List(start) => {
                            newline(&mut doc.text);
                            lists.push(start);
                        }

                        Tag::Item => {
                            newline(&mut doc.text);
                            doc.text.push_str(&"  ".repeat(lists.len().saturating_sub(1)));

                            if let Some(Some(number)) = lists.last_mut() {
                                doc.text.push_str(&format!("{number}. "));
                                *number = number.saturating_add(1);
                            } else {
                                doc.text.push_str("• ");
                            }
                        }

                        _ => {}
                    }
                }

                Event::End(end) => {
                    if open.last().is_some_and(|(tag, _, _)| *tag == end) {
                        let (_, start, style) = open.pop().unwrap();
                        let stop = doc.text.len();
                        let mut native = true;

                        if let Style::Link(url) = &style
                            && (!links || end == End::Image || !safe_link(url))
                        {
                            native = false;

                            if !url.is_empty() && doc.text[start..] != *url {
                                doc.text.push_str(" (");
                                doc.text.push_str(url);
                                doc.text.push(')');
                            }
                        }

                        if native && start < stop && doc.spans.len() < 4096 {
                            doc.spans.push(Span { start, end: stop, style });
                        }
                    }

                    match end {
                        End::List(_) => {
                            lists.pop();
                            newline(&mut doc.text);
                        }

                        End::Item | End::BlockQuote(_) => newline(&mut doc.text),

                        End::TableHead | End::TableRow => {
                            if doc.text.ends_with('\t') {
                                doc.text.pop();
                            }

                            newline(&mut doc.text);
                        }

                        End::TableCell => doc.text.push('\t'),

                        End::Paragraph | End::Heading(_) | End::CodeBlock | End::Table => {
                            newline(&mut doc.text);

                            if lists.is_empty() {
                                doc.text.push('\n');
                            }
                        }

                        _ => {}
                    }
                }

                Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
                    doc.text.push_str(&text)
                }

                Event::Code(text) => {
                    let start = doc.text.len();

                    doc.text.push_str(&text);

                    if !text.is_empty() && doc.spans.len() < 4096 {
                        doc.spans.push(Span { start, end: doc.text.len(), style: Style::Code });
                    }
                }

                Event::SoftBreak | Event::HardBreak => doc.text.push('\n'),

                Event::Rule => {
                    newline(&mut doc.text);
                    doc.text.push_str("—\n");
                }

                Event::TaskListMarker(checked) => {
                    doc.text.push_str(if checked { "[x] " } else { "[ ] " })
                }

                _ => {}
            }

            if doc.text.len() > LIMIT {
                doc.text.truncate(boundary(&doc.text, LIMIT - '…'.len_utf8()));
                doc.text.push('…');
                break;
            }
        }

        let length = doc.text.trim_end_matches('\n').len();

        doc.text.truncate(length);

        for span in &mut doc.spans {
            span.end = span.end.min(length);
        }

        doc.spans.retain(|span| span.start < span.end);
        doc.spans.sort_by_key(|span| (span.start, std::cmp::Reverse(span.end)));
        doc
    }

    pub fn plain(source: &str) -> String {
        let doc = Self::parse(source, false);

        if source.len() <= LIMIT && doc.text == source.trim_end_matches('\n') {
            source.into()
        } else {
            doc.text
        }
    }

    /// Split rendered text, then clip and rebase every span to its chunk.
    pub fn chunks(&self, limit: usize, utf16: bool) -> Vec<Self> {
        let mut result = Vec::new();
        let mut start = 0;
        let mut units = 0;

        for (offset, ch) in self.text.char_indices() {
            let width = if utf16 { ch.len_utf16() } else { 1 };

            if units > 0 && units + width > limit.max(2) {
                result.push(self.slice(start, offset));
                start = offset;
                units = 0;
            }

            units += width;
        }

        result.push(self.slice(start, self.text.len()));
        result
    }

    fn slice(&self, start: usize, end: usize) -> Self {
        Self {
            text: self.text[start..end].into(),
            spans: self
                .spans
                .iter()
                .filter_map(|span| {
                    let mut left = span.start.max(start);
                    let mut right = span.end.min(end);

                    if left < right
                        && matches!(
                            span.style,
                            Style::Bold | Style::Italic | Style::Strike | Style::Heading(_)
                        )
                    {
                        let text = &self.text[left..right];
                        left += text.len() - text.trim_start().len();
                        right -= text.len() - text.trim_end().len();
                    }

                    (left < right).then(|| Span {
                        start: left - start,
                        end: right - start,
                        style: span.style.clone(),
                    })
                })
                .collect(),
        }
    }

    pub fn utf16(&self, span: &Span) -> (usize, usize) {
        (
            self.text[..span.start].encode_utf16().count(),
            self.text[span.start..span.end].encode_utf16().count(),
        )
    }

    pub fn runs(&self) -> Vec<(&str, Vec<&Style>)> {
        let mut boundaries = vec![0, self.text.len()];

        for span in &self.spans {
            boundaries.extend([span.start, span.end]);
        }

        boundaries.sort_unstable();
        boundaries.dedup();
        boundaries
            .windows(2)
            .map(|range| {
                let styles = self
                    .spans
                    .iter()
                    .filter(|span| span.start <= range[0] && span.end >= range[1])
                    .map(|span| &span.style)
                    .collect();

                (&self.text[range[0]..range[1]], styles)
            })
            .collect()
    }
}

pub fn safe_link(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://") || url.starts_with("mailto:"))
        && !url.chars().any(|ch| ch.is_control() || ch.is_whitespace())
}

fn boundary(text: &str, limit: usize) -> usize {
    let mut end = limit.min(text.len());

    while !text.is_char_boundary(end) {
        end -= 1;
    }

    end
}

fn newline(text: &mut String) {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
}

/// A channel supplies only the markup syntax that its transport supports.
pub struct Syntax {
    pub bold: &'static str,
    pub italic: &'static str,
    pub strike: &'static str,
    pub escape: bool,
    pub links: bool,
    pub headings: bool,
    pub language: bool,
}

impl Syntax {
    pub fn chunks(&self, document: &Document, limit: usize) -> Vec<String> {
        let fallback;

        let document = if document.spans.iter().any(
            |span| matches!(&span.style, Style::Link(url) if url.chars().count() + 16 >= limit),
        ) {
            fallback = visible_links(document);
            &fallback
        } else {
            document
        };

        let mut output = Vec::new();

        for part in document.chunks(limit, false) {
            self.fit(&part, limit.max(64), &mut output);
        }

        output
    }

    fn fit(&self, document: &Document, limit: usize, output: &mut Vec<String>) {
        let text = self.render(document);

        if text.chars().count() <= limit {
            output.push(text);
            return;
        }

        let count = document.text.chars().count();

        if count < 2 {
            for part in visible_links(document).chunks(limit / 2, false) {
                output.push(self.render(&part));
            }

            return;
        }

        let middle = document.text.char_indices().nth(count / 2).unwrap().0;

        self.fit(&document.slice(0, middle), limit, output);
        self.fit(&document.slice(middle, document.text.len()), limit, output);
    }

    fn render(&self, document: &Document) -> String {
        let mut output = String::new();
        let mut active: Vec<(String, String)> = Vec::new();
        let mut line_start = true;

        for (text, styles) in document.runs() {
            let linked = self.links && styles.iter().any(|style| matches!(style, Style::Link(_)));

            let code =
                styles.iter().find(|style| !linked && matches!(style, Style::Pre(_) | Style::Code));

            let literal = code.is_some() && !text.contains("```");
            let quote = !literal && styles.iter().any(|style| matches!(style, Style::Quote));

            let heading = styles.iter().find_map(|style| {
                if let Style::Heading(level) = style { Some(*level) } else { None }
            });

            let mut wanted = Vec::new();

            for style in &styles {
                if code.is_some() && !code.is_some_and(|code| *style == *code) {
                    continue;
                }

                let marker = match style {
                    Style::Bold => Some((self.bold.into(), self.bold.into())),
                    Style::Italic => Some((self.italic.into(), self.italic.into())),
                    Style::Strike => Some((self.strike.into(), self.strike.into())),

                    Style::Heading(_) if !self.headings => {
                        Some((self.bold.into(), self.bold.into()))
                    }

                    Style::Code if literal => {
                        let fence = if text.contains('`') { "```" } else { "`" };

                        Some((fence.into(), fence.into()))
                    }

                    Style::Pre(language) if literal => {
                        if self.language {
                            Some((
                                format!("```{language}\n"),
                                if text.ends_with('\n') { "```" } else { "\n```" }.into(),
                            ))
                        } else {
                            Some(("```".into(), "```".into()))
                        }
                    }

                    Style::Link(url) if self.links => {
                        let url = url
                            .replace('(', "%28")
                            .replace(')', "%29")
                            .replace('<', "%3C")
                            .replace('>', "%3E")
                            .replace('\\', "%5C");
                        Some(("[".into(), format!("]({url})")))
                    }

                    _ => None,
                };

                if let Some(marker) = marker
                    && !wanted.contains(&marker)
                {
                    wanted.push(marker);
                }
            }

            let shared = active.iter().zip(&wanted).take_while(|(a, b)| a == b).count();

            for (_, close) in active[shared..].iter().rev() {
                output.push_str(close);
            }

            if line_start {
                if quote {
                    output.push_str("> ");
                }

                if self.headings
                    && let Some(level) = heading
                {
                    output.push_str(&"#".repeat(usize::from(level.min(3))));
                    output.push(' ');
                }
            }

            for (open, _) in &wanted[shared..] {
                output.push_str(open);
            }

            for ch in text.chars() {
                if line_start && quote && ch != '\n' && output.ends_with('\n') {
                    output.push_str("> ");
                }

                if !literal && self.escape && "\\*_~`[]()<>#|".contains(ch) {
                    output.push('\\');
                }

                output.push(ch);
                line_start = ch == '\n';
            }

            active = wanted;
        }

        for (_, close) in active.iter().rev() {
            output.push_str(close);
        }

        output
    }
}

fn visible_links(document: &Document) -> Document {
    let mut text = String::new();
    let mut cursor = 0;

    for span in &document.spans {
        if let Style::Link(url) = &span.style {
            if span.end > cursor {
                text.push_str(&document.text[cursor..span.end]);
            }

            if document.text[span.start..span.end] != *url {
                text.push_str(" (");
                text.push_str(url);
                text.push(')');
            }

            cursor = cursor.max(span.end);

            if text.len() > LIMIT {
                break;
            }
        }
    }

    text.push_str(&document.text[cursor..]);

    if text.len() > LIMIT {
        text.truncate(boundary(&text, LIMIT - '…'.len_utf8()));
        text.push('…');
    }

    Document { text, spans: Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syntax() -> Syntax {
        Syntax {
            bold: "**",
            italic: "*",
            strike: "~~",
            escape: true,
            links: true,
            headings: true,
            language: true,
        }
    }

    #[test]
    fn parses_semantics_and_preserves_native_link_targets() {
        let doc = Document::parse(
            "# Heading\n\n👋 **bold *italic*** ~~old~~ `code` [repo](https://example.com)\n\n> quote\n\n```rust\nx < y\n```",
            true,
        );

        for wanted in [
            Style::Heading(1),
            Style::Bold,
            Style::Italic,
            Style::Strike,
            Style::Code,
            Style::Quote,
            Style::Pre("rust".into()),
            Style::Link("https://example.com".into()),
        ] {
            assert!(doc.spans.iter().any(|span| span.style == wanted), "{wanted:?}");
        }

        let bold = doc.spans.iter().find(|span| span.style == Style::Bold).unwrap();

        assert_eq!(&doc.text[bold.start..bold.end], "bold italic");
        assert!(doc.utf16(bold).0 < bold.start);
        assert!(!doc.text.contains("https://"));
        assert_eq!(Document::plain("[repo](https://example.com)"), "repo (https://example.com)");

        assert_eq!(
            Document::plain("![image](https://example.com/a.png)"),
            "image (https://example.com/a.png)"
        );

        assert_eq!(Document::plain("<https://example.com>"), "https://example.com");
    }

    #[test]
    fn plain_fallback_retains_structure_literals_and_unsupported_urls() {
        assert_eq!(Document::plain("hello 👋\n"), "hello 👋\n");
        assert_eq!(Document::plain(""), "");
        assert_eq!(Document::plain("**unfinished"), "**unfinished");
        assert_eq!(Document::plain("<b>literal</b>"), "<b>literal</b>");

        assert_eq!(
            Document::plain(
                "3. three\n4. four\n   - inner\n\n- [x] done\n\n---\n\n| A | B |\n|---|---|\n| 1 | 2 |"
            ),
            "3. three\n4. four\n  • inner\n• [x] done\n—\nA\tB\n1\t2"
        );

        assert_eq!(Document::plain("one  \ntwo\nthree"), "one\ntwo\nthree");
        assert_eq!(Document::parse("[bad](javascript:alert)", true).text, "bad (javascript:alert)");
        assert!(Document::parse("[bad](javascript:alert)", true).spans.is_empty());
        assert!(safe_link("mailto:me@example.com"));
        assert!(safe_link("http://example.com"));
        assert!(!safe_link("https://example.com/\n"));

        assert!(
            Document::parse("    indented code\n", true)
                .spans
                .iter()
                .any(|span| span.style == Style::Pre(String::new()))
        );
    }

    #[test]
    fn splits_spans_on_unicode_boundaries_and_reconstructs_text() {
        let doc = Document::parse("**🦀a🦀b🦀**", true);
        let parts = doc.chunks(3, true);

        assert_eq!(parts.iter().map(|part| part.text.as_str()).collect::<String>(), doc.text);
        assert!(parts.iter().all(|part| part.text.encode_utf16().count() <= 3));

        assert!(
            parts
                .iter()
                .all(|part| part.spans[0].start == 0 && part.spans[0].end == part.text.len())
        );

        assert_eq!(Document::default().chunks(0, true).len(), 1);
        assert_eq!(doc.chunks(3, false).len(), 2);

        let doc = Document::parse("**word  next**", true);
        let parts = doc.chunks(5, false);

        assert_eq!(parts[0].text, "word ");
        assert_eq!(parts[0].spans[0].end, 4);
        assert_eq!(parts[1].spans[0].start, 1);
    }

    #[test]
    fn markup_keeps_nested_styles_escapes_literals_and_preserves_code() {
        let doc = Document::parse(
            "**bold *italic*** [repo](https://example.com/a\\(b\\))\n\n> quoted\n> second\n\n# Heading\n\n```rs\nx * y\n```",
            true,
        );

        let output = syntax().chunks(&doc, 2000).join("");

        assert!(output.contains("**bold *italic***"), "{output}");
        assert!(output.contains("[repo](https://example.com/a%28b%29)"), "{output}");
        assert!(output.contains("> quoted\n> second"), "{output}");
        assert!(output.contains("# Heading"));
        assert!(output.contains("```rs\nx * y"));

        assert_eq!(
            syntax().chunks(&Document::parse("\\*literal\\* & <b>", true), 2000),
            vec!["\\*literal\\* & \\<b\\>"]
        );

        assert_eq!(syntax().chunks(&Document::parse("`x * y`", true), 2000), vec!["`x * y`"]);
    }

    #[test]
    fn custom_markup_splits_balanced_styles_and_degrades_unsupported_links() {
        let syntax = Syntax {
            bold: "*",
            italic: "_",
            strike: "~",
            escape: false,
            links: false,
            headings: false,
            language: false,
        };

        let doc = Document::parse(
            "# Header\n\n**bold** *italic* ~~old~~ `code`\n\n```rs\ncode\n```\n\n[repo](https://example.com)",
            false,
        );

        let output = syntax.chunks(&doc, 4000).join("");

        assert!(output.contains("*Header*"));
        assert!(output.contains("*bold* _italic_ ~old~ `code`"));
        assert!(output.contains("```code"));
        assert!(output.contains("repo (https://example.com)"));

        let doc = Document::parse(&format!("**{}**", "🦀".repeat(300)), true);
        let chunks = syntax.chunks(&doc, 64);

        assert!(chunks.len() > 1);

        assert!(chunks.iter().all(|part| part.chars().count() <= 64
            && part.starts_with('*')
            && part.ends_with('*')));
    }

    #[test]
    fn bounds_expansion_and_pathological_markup() {
        let source = format!(
            "{}\n\n{}\n\n[s]: https://example.com/{}",
            "x".repeat(LIMIT - 10000),
            "[source][s] ".repeat(100),
            "é".repeat(1024)
        );

        let doc = Document::parse(&source, false);

        assert!(doc.text.len() <= LIMIT);
        assert!(doc.text.ends_with('…'));
        assert!(Document::parse(&"🦀".repeat(LIMIT), true).text.len() <= LIMIT);
        assert!(Document::parse(&"**bold** ".repeat(5000), true).spans.len() <= 4096);

        let doc = Document::parse(&format!("[x](https://example.com/{})", "a".repeat(1000)), true);

        let parts = syntax().chunks(&doc, 64);

        assert!(parts.iter().all(|part| part.chars().count() <= 64));
        assert!(parts.join("").contains(&format!("https://example.com/{}", "a".repeat(1000))));
        assert!(syntax().chunks(&Document::parse("`` ` ``", true), 64)[0].contains('`'));
        assert!(syntax().chunks(&Document::parse("````\n```\n````", true), 64)[0].contains("\\`"));
    }
}
