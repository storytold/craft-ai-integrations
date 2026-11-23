//! Mask decoding and RGB PNG encoding for [`super`], keeping the same Craft mask convention as
//! `craft-ai-media` without adding network crates to `craft-ai-engine-rten`.

use std::io::Cursor;

use craft_ai_core::{AiError, ErrorKind};
use png::{BitDepth, ColorType, Transformations};

use crate::picture::{MAX_PIXELS, MAX_SIDE};

/// Maximum decoded PNG bytes accepted for a mask.
const MAX_MASK_ALLOC: usize = 900 * 1024 * 1024;
const PNG_START: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

const PAINT_BY_LUMA: u8 = 1;
const PAINT_BY_ALPHA: u8 = 2;

fn bad_mask() -> AiError {
    AiError::invalid("The mask could not be read. Use a PNG image.")
}

fn record_row(row: &[u8], channels: usize, width: usize, marks: &mut Vec<u8>) -> bool {
    let mut translucent = false;
    for px in row.chunks_exact(channels).take(width) {
        let alpha = if channels.is_multiple_of(2) {
            px.last().copied().unwrap_or(255)
        } else {
            255
        };
        translucent |= alpha < 255;
        let ch = |i: usize| u32::from(px.get(i).copied().unwrap_or(0));
        let luma = if channels >= 3 {
            (ch(0) * 299 + ch(1) * 587 + ch(2) * 114) / 1000
        } else {
            ch(0)
        };
        let mut mark = 0;
        if luma > 127 {
            mark |= PAINT_BY_LUMA;
        }
        if alpha < 128 {
            mark |= PAINT_BY_ALPHA;
        }
        marks.push(mark);
    }
    translucent
}

/// Decodes a Craft mask PNG and checks that it matches `(expected_width, expected_height)`.
///
/// An opaque mask marks pixels brighter than 127 (`255` = remove, `0` = keep); a mask with any
/// translucent pixel marks pixels whose alpha is below 128 (`transparent` = remove, as OpenAI
/// edits do). Refuses a mask that marks nothing at all.
pub(crate) fn decode_mask(
    data: &[u8],
    expected_width: usize,
    expected_height: usize,
) -> Result<Vec<u8>, AiError> {
    if !data.starts_with(&PNG_START) {
        return Err(bad_mask());
    }
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(data),
        png::Limits {
            bytes: MAX_MASK_ALLOC,
        },
    );
    decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| bad_mask())?;
    let (w, h) = (
        u64::from(reader.info().width),
        u64::from(reader.info().height),
    );
    if w == 0 || h == 0 || w * h > MAX_PIXELS || w > MAX_SIDE || h > MAX_SIDE {
        return Err(AiError::invalid(
            "The mask is too large (over 100 megapixels, or a side over 65,535 pixels).",
        ));
    }
    let (width, height) = (w as usize, h as usize);
    if (width, height) != (expected_width, expected_height) {
        return Err(AiError::invalid(
            "The mask must be the same size as the picture.",
        ));
    }
    let channels = match reader.output_color_type() {
        (_, depth) if depth != BitDepth::Eight => return Err(bad_mask()),
        (ColorType::Grayscale, _) => 1,
        (ColorType::GrayscaleAlpha, _) => 2,
        (ColorType::Rgb, _) => 3,
        (ColorType::Rgba, _) => 4,
        (ColorType::Indexed, _) => return Err(bad_mask()),
    };
    let count = width.checked_mul(height).ok_or_else(bad_mask)?;
    let row_len = width.checked_mul(channels).ok_or_else(bad_mask)?;
    let mut marks = Vec::new();
    marks.try_reserve_exact(count).map_err(|_| bad_mask())?;
    let mut translucent = false;
    if reader.info().interlaced {
        let mut buffer = vec![0u8; reader.output_buffer_size().ok_or_else(bad_mask)?];
        let frame = reader.next_frame(&mut buffer).map_err(|_| bad_mask())?;
        let pixels = buffer.get(..frame.buffer_size()).ok_or_else(bad_mask)?;
        for row in pixels.chunks_exact(row_len).take(height) {
            translucent |= record_row(row, channels, width, &mut marks);
        }
    } else {
        while let Some(row) = reader.next_row().map_err(|_| bad_mask())? {
            translucent |= record_row(row.data(), channels, width, &mut marks);
        }
    }
    if marks.len() != count {
        return Err(bad_mask());
    }
    let bit = if translucent {
        PAINT_BY_ALPHA
    } else {
        PAINT_BY_LUMA
    };
    let mut any_marked = false;
    for m in &mut marks {
        *m = if *m & bit != 0 { 255 } else { 0 };
        any_marked |= *m == 255;
    }
    if !any_marked {
        return Err(AiError::invalid("The mask marks nothing to remove."));
    }
    Ok(marks)
}

/// Encodes row-major 8-bit `rgb` (`width * height * 3` bytes) as a lossless RGB PNG.
pub(crate) fn encode_rgb(rgb: &[u8], width: usize, height: usize) -> Result<Vec<u8>, AiError> {
    fn fail<E>(_: E) -> AiError {
        AiError::new(
            ErrorKind::Internal,
            "The filled picture could not be saved.",
        )
    }
    let expected = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(3))
        .ok_or_else(|| fail("overflow"))?;
    if width == 0 || height == 0 || rgb.len() != expected {
        return Err(fail("wrong buffer length"));
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(
        &mut out,
        u32::try_from(width).map_err(fail)?,
        u32::try_from(height).map_err(fail)?,
    );
    encoder.set_color(ColorType::Rgb);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(fail)?;
    writer.write_image_data(rgb).map_err(fail)?;
    writer.finish().map_err(fail)?;
    Ok(out)
}
