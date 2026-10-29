//! Posters and thumbnails of a deck, decoded from its verified `deck.mp4` in one forward
//! pass: a full-size JPEG (quality 92) and a 320 px wide one per stop.
use std::{
    fs,
    io::{BufWriter, Write},
    path::Path,
};

use fframes_media::FFmpegDecoder;
use image::{RgbImage, codecs::jpeg::JpegEncoder, imageops::FilterType};

use super::manifest::image_name;
use crate::services::export_service::ExportProgress;

pub const POSTER_QUALITY: u8 = 92;
pub const THUMB_QUALITY: u8 = 85;
pub const THUMB_WIDTH: u32 = 320;

/// The size of the video's frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

/// Decodes `frames` (sorted video frames at `fps`) from `video` and writes
/// `posters/NNNN.jpg` and `thumbs/NNNN.jpg` (1-based) into `folder`. Checks `cancelled`
/// at most one second of video apart while decoding. The decoder is dropped before returning.
pub fn extract(
    video: &Path,
    fps: u32,
    frames: &[usize],
    folder: &Path,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(ExportProgress),
) -> Result<FrameSize, String> {
    let posters = folder.join("posters");
    let thumbs = folder.join("thumbs");
    for dir in [&posters, &thumbs] {
        fs::create_dir(dir)
            .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
    }
    let fps_usize = usize::try_from(fps).map_err(|_| "invalid frame rate".to_owned())?;
    if fps == 0 {
        return Err("the deck needs a frame rate".into());
    }
    let mut decoder = unsafe { FFmpegDecoder::new(video, fps_usize, 1) }
        .map_err(|error| format!("deck video could not be opened: {error:?}"))?;
    let size = FrameSize {
        width: decoder.get_stream_width(),
        height: decoder.get_stream_height(),
    };
    if let (Some(last_stop), Some(last_offset)) = (frames.last(), decoder.get_last_frame_offset()) {
        // The deck's final stop is the timeline's last frame; a video of another length
        // was rendered from another timeline than the slides were laid out on.
        let last_frame = usize::try_from(last_offset).unwrap_or(0);
        if last_frame.abs_diff(*last_stop) > 1 {
            return Err(format!(
                "the rendered video has {} frames but the slides were laid out on {}; wait for the preview to update and export again",
                last_frame + 1,
                last_stop + 1
            ));
        }
    }
    let total = frames.len();
    let mut decoded_offset = -1;
    for (position, &frame) in frames.iter().enumerate() {
        if cancelled() {
            return Err("export cancelled".into());
        }
        let offset = i64::try_from(frame).map_err(|_| "stop frame out of range".to_owned())?;
        // A first poster near the end must not decode the entire prefix without polling.
        while decoded_offset < offset {
            if cancelled() {
                return Err("export cancelled".into());
            }
            let next = decoded_offset.saturating_add(i64::from(fps)).min(offset);
            let decoded = unsafe { decoder.decode_up_to(next) }
                .map_err(|error| format!("deck frame {next} could not be decoded: {error:?}"))?;
            if !decoded {
                return Err(format!("the deck video ends before frame {next}"));
            }
            decoded_offset = next;
        }
        let raw = decoder.get_raw_frame();
        // The first frame with pts >= the stop: an exact stop has the stop's timestamp.
        let at = f64::from(unsafe { raw.timestamp_seconds() }) * f64::from(fps);
        if !at.is_finite() || (at - frame as f64).abs() >= 0.5 {
            return Err(format!(
                "the deck video shows frame {at:.2} where frame {frame} was expected; its timestamps are shifted"
            ));
        }
        let pixels = unsafe { raw.convert_last_decoded_frame_into_svg_image(None) }
            .map_err(|error| format!("deck frame {frame} could not be converted: {error:?}"))?;
        let rgb = rgba_to_rgb(&pixels.data, pixels.width, pixels.height)?;
        let name = image_name(position + 1);
        write_jpeg(&posters.join(&name), &rgb, POSTER_QUALITY)?;
        write_jpeg(&thumbs.join(&name), &thumbnail(&rgb), THUMB_QUALITY)?;
        progress(ExportProgress::Posters {
            done: position + 1,
            total,
        });
    }
    drop(decoder);
    Ok(size)
}

fn rgba_to_rgb(rgba: &[u8], width: u32, height: u32) -> Result<RgbImage, String> {
    let pixels = width as usize * height as usize;
    if width == 0 || height == 0 || rgba.len() < pixels * 4 {
        return Err("a decoded deck frame has no pixels".into());
    }
    let rgb = rgba[..pixels * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[red, green, blue, _]| [red, green, blue])
        .collect();
    RgbImage::from_raw(width, height, rgb).ok_or_else(|| "a decoded deck frame is malformed".into())
}

/// The frame scaled to [`THUMB_WIDTH`] wide; smaller frames stay as they are.
pub fn thumbnail(frame: &RgbImage) -> RgbImage {
    if frame.width() <= THUMB_WIDTH {
        return frame.clone();
    }
    let height = ((u64::from(frame.height()) * u64::from(THUMB_WIDTH)
        + u64::from(frame.width()) / 2)
        / u64::from(frame.width()))
    .max(1) as u32;
    image::imageops::resize(frame, THUMB_WIDTH, height, FilterType::Triangle)
}

pub fn write_jpeg(path: &Path, image: &RgbImage, quality: u8) -> Result<(), String> {
    let file = fs::File::create_new(path)
        .map_err(|error| format!("could not create {}: {error}", path.display()))?;
    let mut writer = BufWriter::new(file);
    JpegEncoder::new_with_quality(&mut writer, quality)
        .encode_image(image)
        .map_err(|error| format!("could not encode {}: {error}", path.display()))?;
    writer
        .flush()
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnails_are_320_wide_and_keep_the_aspect() {
        let frame = RgbImage::new(1920, 1080);
        let thumb = thumbnail(&frame);
        assert_eq!((thumb.width(), thumb.height()), (320, 180));
        let small = RgbImage::new(200, 100);
        assert_eq!(thumbnail(&small).dimensions(), (200, 100));
    }

    #[test]
    fn rgba_drops_alpha() {
        let rgb = rgba_to_rgb(&[1, 2, 3, 255, 4, 5, 6, 0], 2, 1).unwrap();
        assert_eq!(rgb.into_raw(), [1, 2, 3, 4, 5, 6]);
        assert!(rgba_to_rgb(&[1, 2, 3], 1, 1).is_err());
    }
}
