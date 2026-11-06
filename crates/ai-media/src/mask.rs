//! Masks for the inpainting adapters: reading the craft mask, checking sizes, and writing the
//! black-and-white mask a service is sent or shown.
//!
//! The craft mask is a PNG, white where to paint and black where the picture stays (as Select
//! subject produces and PhotoCraft sends). A mask that carries transparency is read the other
//! common way instead, transparent where to paint, as OpenAI's edits take it. Either way the result
//! is a binary 8-bit grayscale PNG, white where to paint, so a stray grey or an alpha channel
//! cannot be read differently by the service.

use craft_ai_core::{AiError, ErrorKind, InputRole, Payload, Request};
use png::{BitDepth, ColorType, Transformations};
use std::io::Cursor;

mod instruction;
pub use instruction::fill_instruction;

/// The largest pictures an adapter accepts: a side and a total pixel count. `service` names the
/// adapter in its errors ("FLUX Fill needs a mask").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub service: &'static str,
    pub max_side: u32,
    pub max_pixels: u64,
}

impl Bounds {
    /// Whether a `width` × `height` picture is non-empty and within the bounds.
    pub fn fits(&self, width: u32, height: u32) -> bool {
        width != 0
            && height != 0
            && width <= self.max_side
            && height <= self.max_side
            && u64::from(width) * u64::from(height) <= self.max_pixels
    }

    /// [`Bounds::fits`] as a result, with the error a person reads.
    pub fn check(&self, width: u32, height: u32) -> Result<(), AiError> {
        if self.fits(width, height) {
            return Ok(());
        }
        Err(AiError::invalid(format!(
            "pictures for {} may be at most {} pixels a side and {} megapixels",
            self.service,
            self.max_side,
            self.max_pixels / (1024 * 1024)
        )))
    }
}

/// The inline bytes of the request's `role` input; `what` names the input in errors.
pub fn inline<'a>(
    request: &'a Request,
    bounds: Bounds,
    role: InputRole,
    what: &str,
) -> Result<&'a [u8], AiError> {
    let adapter = bounds.service;
    let input = request
        .inputs
        .iter()
        .find(|i| i.role == role)
        .ok_or_else(|| AiError::invalid(format!("{adapter} needs a {what}")))?;
    match &input.payload {
        Payload::Inline { data, .. } => Ok(data),
        _ => Err(AiError::unsupported(format!(
            "the {what} must be inline image data; this adapter does not read files or URLs"
        ))),
    }
}

/// A PNG's size from its header, without decoding the pixels.
pub fn png_size(data: &[u8]) -> Option<(u32, u32)> {
    let reader = png::Decoder::new(Cursor::new(data)).read_info().ok()?;
    Some((reader.info().width, reader.info().height))
}

/// Decodes every pixel of a PNG within `bounds` and returns its size, so a truncated or corrupt
/// picture is refused here, before it is uploaded or paid for. `what` names it in errors. Rows are
/// read one at a time, so only one row is held, whatever the picture's size.
pub fn png_checked(data: &[u8], bounds: Bounds, what: &str) -> Result<(u32, u32), AiError> {
    let bad = || AiError::invalid(format!("the {what} is not a readable PNG image"));
    if !matches!(crate::sniff::media_type(data), Some(("image/png", _))) {
        return Err(AiError::invalid(format!("the {what} must be a PNG image")));
    }
    let limit = png::Limits {
        bytes: usize::try_from(bounds.max_pixels.saturating_mul(8)).unwrap_or(usize::MAX),
    };
    let mut reader = png::Decoder::new_with_limits(Cursor::new(data), limit)
        .read_info()
        .map_err(|_| bad())?;
    let (width, height) = (reader.info().width, reader.info().height);
    bounds.check(width, height)?;
    let mut rows = 0u32;
    while reader.next_row().map_err(|_| bad())?.is_some() {
        rows = rows.saturating_add(1);
    }
    // An interlaced picture yields each row once per pass, so only a shortfall is an error.
    if rows < height {
        return Err(bad());
    }
    Ok((width, height))
}

fn bad_mask() -> AiError {
    AiError::invalid("the mask is not a readable PNG image")
}

/// Reads the craft mask and returns an opaque 8-bit grayscale PNG, white where to paint and black
/// where to keep, with its size. A mask outside `bounds`, or one that marks nothing to fill, is
/// refused.
pub fn binary(data: &[u8], bounds: Bounds) -> Result<(Vec<u8>, u32, u32), AiError> {
    if !matches!(crate::sniff::media_type(data), Some(("image/png", _))) {
        return Err(AiError::invalid("the mask must be a PNG image"));
    }
    let limit = png::Limits {
        bytes: usize::try_from(bounds.max_pixels.saturating_mul(8)).unwrap_or(usize::MAX),
    };
    let mut decoder = png::Decoder::new_with_limits(Cursor::new(data), limit);
    decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| bad_mask())?;
    let (width, height) = (reader.info().width, reader.info().height);
    bounds.check(width, height)?;
    let mut buffer = vec![0u8; reader.output_buffer_size().ok_or_else(bad_mask)?];
    let frame = reader.next_frame(&mut buffer).map_err(|_| bad_mask())?;
    let channels = match frame.color_type {
        ColorType::Grayscale => 1,
        ColorType::GrayscaleAlpha => 2,
        ColorType::Rgb => 3,
        ColorType::Rgba => 4,
        ColorType::Indexed => return Err(bad_mask()),
    };
    let count = width as usize * height as usize;
    let pixels = buffer.get(..frame.buffer_size()).ok_or_else(bad_mask)?;
    if frame.bit_depth != BitDepth::Eight || pixels.len() < count * channels {
        return Err(bad_mask());
    }
    let pixels = pixels.chunks_exact(channels).take(count);
    let has_alpha = channels % 2 == 0;
    let alpha = |p: &[u8]| p.last().copied().unwrap_or(255);
    let by_alpha = has_alpha && pixels.clone().any(|p| alpha(p) < 255);
    let gray: Vec<u8> = pixels
        .map(|p| {
            let paint = if by_alpha {
                alpha(p) < 128
            } else {
                let c = |i: usize| u32::from(p.get(i).copied().unwrap_or(0));
                let luma = if channels >= 3 {
                    (c(0) * 299 + c(1) * 587 + c(2) * 114) / 1000
                } else {
                    c(0)
                };
                luma > 127
            };
            if paint { 255 } else { 0 }
        })
        .collect();
    if !gray.contains(&255) {
        return Err(AiError::invalid("the mask marks nothing to fill"));
    }
    Ok((encode_gray(&gray, width, height)?, width, height))
}

/// An 8-bit grayscale PNG of `gray`, which holds `width * height` samples.
pub fn encode_gray(gray: &[u8], width: u32, height: u32) -> Result<Vec<u8>, AiError> {
    fn fail<E>(_: E) -> AiError {
        AiError::new(ErrorKind::Internal, "the mask could not be prepared")
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(ColorType::Grayscale);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(fail)?;
    writer.write_image_data(gray).map_err(fail)?;
    writer.finish().map_err(fail)?;
    Ok(out)
}
