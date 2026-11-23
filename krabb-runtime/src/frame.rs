//! Decodes the first frame of an inbound image within fixed memory and size bounds.

use std::io::Cursor;

use image::{ImageFormat, ImageReader, Limits};

const SIDE: u32 = 4096;
const MEMORY: u64 = 64 * 1024 * 1024;

pub(crate) fn first(bytes: &[u8], limit: usize) -> Option<Vec<u8>> {
    let mut limits = Limits::default();

    limits.max_image_width = Some(SIDE);
    limits.max_image_height = Some(SIDE);
    limits.max_alloc = Some(MEMORY);

    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    reader.limits(limits);

    let image = reader.decode().ok()?;
    let mut output = Cursor::new(Vec::new());

    image.write_to(&mut output, ImageFormat::Png).ok()?;

    let output = output.into_inner();
    (output.len() <= limit).then_some(output)
}
