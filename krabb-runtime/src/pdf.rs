//! Extracts bounded text from inbound PDF attachments.

use std::{path::Path, time::Duration};

const TIME: Duration = Duration::from_secs(30);

pub(crate) const MAGIC: &[u8] = b"%PDF-";

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Text {
    Found(String),
    Empty,
    Unreadable,
}

pub(crate) async fn read(path: &Path) -> Text {
    let Ok(bytes) = std::fs::read(path) else {
        return Text::Unreadable;
    };

    let task = tokio::task::spawn_blocking(move || pdf_extract::extract_text_from_mem(&bytes));

    match tokio::time::timeout(TIME, task).await {
        Ok(Ok(Ok(text))) => found(&text),

        Ok(Ok(Err(error))) => {
            tracing::warn!(%error, "PDF text extraction failed.");
            Text::Unreadable
        }

        Ok(Err(_)) => {
            tracing::warn!("PDF text extraction stopped unexpectedly.");
            Text::Unreadable
        }

        Err(_) => {
            tracing::warn!("PDF text extraction timed out.");
            Text::Unreadable
        }
    }
}

fn found(text: &str) -> Text {
    let text = text
        .chars()
        .map(|character| match character {
            '\n' | '\t' => character,
            '\u{c}' => '\n',
            character if character.is_control() => ' ',
            character => character,
        })
        .collect::<String>();

    let text = text.trim();

    if text.is_empty() { Text::Empty } else { Text::Found(text.to_owned()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_extracted_text() {
        assert_eq!(
            found("Page one\u{c}Page\u{0}two\r\n"),
            Text::Found("Page one\nPage two".into())
        );
        assert_eq!(found(" \n\u{c} "), Text::Empty);
    }

    #[tokio::test]
    async fn rejects_damaged_documents() {
        let root = std::env::temp_dir().join(format!("krabb-pdf-{}", std::process::id()));
        let path = root.join("broken.pdf");

        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&path, b"%PDF-1.7\nnot a document").unwrap();

        assert_eq!(read(&path).await, Text::Unreadable);
        assert_eq!(read(&root.join("missing.pdf")).await, Text::Unreadable);

        let _ = std::fs::remove_dir_all(root);
    }
}
