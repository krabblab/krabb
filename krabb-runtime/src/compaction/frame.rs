use krabb_core::types::ModelRequest;
use std::io::{self, Write};

/// Leave room for the RPC envelope and generation metadata without allocating a second frame.
pub(super) fn fits(input: &ModelRequest) -> bool {
    let mut frame = Frame(krabb_core::jsonl::MAX - 256);
    serde_json::to_writer(&mut frame, input).is_ok()
}

struct Frame(usize);

impl Write for Frame {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self.0.checked_sub(bytes.len()).ok_or_else(|| io::Error::other("Frame limit"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_serialized_bytes_including_escaping_without_a_frame_allocation() {
        let mut frame = Frame(10);

        serde_json::to_writer(&mut frame, "\n\n").unwrap();

        assert_eq!(frame.0, 4);
        assert!(serde_json::to_writer(&mut frame, "too long").is_err());

        frame.flush().unwrap();
    }
}
