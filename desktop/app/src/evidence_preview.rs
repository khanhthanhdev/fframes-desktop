//! Bounded PNG decoding for task-owned review evidence thumbnails.
use gpui::RenderImage;
use image::{Frame, ImageFormat, ImageReader};
use smallvec::SmallVec;
use std::{io::Cursor, sync::Arc};
use thiserror::Error;

/// The task artifact store already bounds each PNG; keep the combined review preview
/// payload bounded as well.
pub(crate) const MAX_ENCODED_BYTES: usize = 8 * 1024 * 1024;
/// Reject unexpectedly large PNG dimensions before asking the decoder to allocate pixels.
const MAX_DECODED_PIXELS: u64 = 3_000_000;
/// Review cards are deliberately small; this is the maximum decoded thumbnail size.
const MAX_THUMBNAIL_SIZE: (u32, u32) = (480, 270);

#[derive(Debug, Error)]
pub(crate) enum PreviewError {
    #[error("PNG exceeds the review preview byte limit")]
    TooLarge,
    #[error("PNG dimensions do not match the rendered evidence metadata")]
    DimensionsMismatch,
    #[error("PNG dimensions exceed the review preview pixel limit")]
    TooManyPixels,
    #[error("invalid evidence PNG: {0}")]
    Decode(#[from] image::ImageError),
}

/// Decodes a validated task-owned PNG into a small GPUI image. The input dimension
/// check happens before full decode, and the returned allocation is capped at 480×270.
pub(crate) fn decode_png(
    bytes: &[u8],
    expected_dimensions: (u32, u32),
) -> Result<(Arc<RenderImage>, usize), PreviewError> {
    if bytes.len() > MAX_ENCODED_BYTES {
        return Err(PreviewError::TooLarge);
    }
    let reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
    let dimensions = reader.into_dimensions()?;
    if dimensions != expected_dimensions || dimensions.0 == 0 || dimensions.1 == 0 {
        return Err(PreviewError::DimensionsMismatch);
    }
    if u64::from(dimensions.0) * u64::from(dimensions.1) > MAX_DECODED_PIXELS {
        return Err(PreviewError::TooManyPixels);
    }

    let image = image::load_from_memory_with_format(bytes, ImageFormat::Png)?;
    let thumbnail = image::imageops::thumbnail(&image, MAX_THUMBNAIL_SIZE.0, MAX_THUMBNAIL_SIZE.1);
    let decoded_bytes = thumbnail.as_raw().len();
    let render_image = RenderImage::new(SmallVec::from_const([Frame::new(thumbnail)]));
    Ok((Arc::new(render_image), decoded_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = ImageBuffer::from_pixel(width, height, Rgba([20_u8, 40, 60, 255]));
        let mut bytes = Cursor::new(Vec::new());
        image
            .write_to(&mut bytes, ImageFormat::Png)
            .expect("write test PNG");
        bytes.into_inner()
    }

    #[test]
    fn decode_png_checks_dimensions_and_downscales() {
        let bytes = png(960, 540);
        let (_image, decoded_bytes) = decode_png(&bytes, (960, 540)).expect("decode PNG");
        assert!(decoded_bytes <= 480 * 270 * 4);
        assert!(matches!(
            decode_png(&bytes, (1920, 1080)),
            Err(PreviewError::DimensionsMismatch)
        ));
    }

    #[test]
    fn decode_png_rejects_dimensions_that_exceed_the_pixel_limit() {
        let bytes = png(2_000, 2_000);
        assert!(matches!(
            decode_png(&bytes, (2_000, 2_000)),
            Err(PreviewError::TooManyPixels)
        ));
    }
}
