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

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([200, 30, 30, 255]));
        let mut output = Cursor::new(Vec::new());

        image::DynamicImage::ImageRgba8(image).write_to(&mut output, format).unwrap();
        output.into_inner()
    }

    #[test]
    fn converts_the_first_frame_to_png() {
        let png = first(&encode(3, 2, ImageFormat::Gif), 1024 * 1024).unwrap();
        let decoded = image::load_from_memory(&png).unwrap();

        assert!(png.starts_with(b"\x89PNG"));
        assert_eq!((decoded.width(), decoded.height()), (3, 2));
    }

    #[test]
    fn rejects_oversized_or_invalid_images() {
        assert!(first(&encode(SIDE + 1, 1, ImageFormat::Gif), usize::MAX).is_none());
        assert!(first(&encode(64, 64, ImageFormat::Gif), 16).is_none());
        assert!(first(b"GIF89a broken", usize::MAX).is_none());
    }
}
