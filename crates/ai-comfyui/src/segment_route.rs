//! The generic `image.segment` route on ComfyUI: what an imported selection graph must satisfy
//! before an app sees its answer, so it speaks the same contract as the built-in engines
//! (`craft_ai_core::segment`).
//!
//! A graph maps a text prompt and one source picture to an image that is the mask. This module
//! makes that image trustworthy:
//! - the mask must be the size of the picture (or turned a quarter, for a JPEG that carries a turn,
//!   which ComfyUI's image loader applies), or the answer is refused;
//! - it is reduced to one 8-bit channel, white where selected, whatever colour type the graph
//!   saved;
//! - a mask with no pixel above half is an empty mask and `matched: false`, never a failed job;
//! - a repeat of the same picture reuses the file already in ComfyUI's scratch area, keyed by the
//!   picture's bytes.
//!
//! The graph's output is read inline and bounded, so a hostile or broken graph cannot ask for more
//! memory than a 100 megapixel mask needs.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Cursor;

use craft_ai_core::job::{Event, Output};
use craft_ai_core::segment::{self, Found};
use craft_ai_core::{AiError, ErrorKind, Payload};
use png::{BitDepth, ColorType, Transformations};

use crate::input::Image;

/// Pictures and masks larger than this are refused before any pixel is read.
const MAX_PIXELS: u64 = 100_000_000;

/// The most memory the mask decoder may ask for.
const MAX_ALLOC: usize = 900 * 1024 * 1024;

/// The size a mask has to be, taken from the picture sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Size {
    pub width: u32,
    pub height: u32,
    /// A JPEG may carry a turn that the loader applies, so its mask may come back turned.
    turnable: bool,
}

impl Size {
    /// Whether a mask of `width` by `height` fits this picture.
    fn accepts(&self, width: u32, height: u32) -> bool {
        (width == self.width && height == self.height)
            || (self.turnable && width == self.height && height == self.width)
    }
}

/// A picture's identity for the upload cache: a hash of its bytes and their length.
pub(crate) type CacheKey = (u64, usize);

/// What lets a repeat of the same picture reuse its upload.
pub(crate) fn cache_key(image: &Image) -> CacheKey {
    let mut hasher = DefaultHasher::new();
    image.data.hash(&mut hasher);
    (hasher.finish(), image.data.len())
}

/// The size of `image`, read from its header, or a refusal when it is empty or over the limit.
pub(crate) fn size_of(image: &Image) -> Result<Size, AiError> {
    let (width, height, turnable) = match image.media_type.as_str() {
        "image/png" => png_size(&image.data).map(|(w, h)| (w, h, false)),
        "image/jpeg" => jpeg_size(&image.data).map(|(w, h)| (w, h, true)),
        _ => None,
    }
    .ok_or_else(|| AiError::invalid("the picture's size could not be read, so nothing was sent"))?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(AiError::invalid(
            "the picture is too large to select from (over 100 megapixels)",
        ));
    }
    Ok(Size {
        width,
        height,
        turnable,
    })
}

/// Width and height from a PNG's header chunk.
fn png_size(data: &[u8]) -> Option<(u32, u32)> {
    let reader = png::Decoder::new(Cursor::new(data)).read_info().ok()?;
    Some((reader.info().width, reader.info().height))
}

/// Width and height from a JPEG's first start-of-frame marker, walking the segments before it.
fn jpeg_size(data: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    loop {
        let [first, marker] = data.get(at..at + 2)? else {
            return None;
        };
        if *first != 0xFF {
            return None;
        }
        match *marker {
            0xFF => at += 1,
            0x01 | 0xD0..=0xD9 => at += 2,
            0xC0..=0xCF if !matches!(*marker, 0xC4 | 0xC8 | 0xCC) => {
                let [h0, h1, w0, w1] = data.get(at + 5..at + 9)? else {
                    return None;
                };
                let height = u32::from(u16::from_be_bytes([*h0, *h1]));
                let width = u32::from(u16::from_be_bytes([*w0, *w1]));
                return Some((width, height));
            }
            _ => {
                let [l0, l1] = data.get(at + 2..at + 4)? else {
                    return None;
                };
                at += 2 + usize::from(u16::from_be_bytes([*l0, *l1]));
            }
        }
    }
}

/// What went wrong with a mask, in words that do not repeat anything the graph sent.
fn bad_mask() -> AiError {
    AiError::new(
        ErrorKind::Provider,
        "The workflow's mask could not be read as a picture.",
    )
}

/// Turns the one picture a graph produced into the contract's answer: the mask as an 8-bit
/// grayscale PNG at the picture's size, then the [`Found`] result.
pub(crate) fn answer(event: Event, size: &Size) -> Result<Vec<Event>, AiError> {
    let Event::Output(Output {
        payload: Payload::Inline { media_type, data },
        ..
    }) = event
    else {
        return Err(bad_mask());
    };
    if media_type.as_str() != "image/png" {
        return Err(AiError::unsupported(
            "A selection workflow must save its mask as a PNG.",
        ));
    }
    let (mask, matched) = finish(&data, size)?;
    let found = Found {
        matched,
        score: None,
        name: None,
    };
    Ok(vec![
        Event::Output(Output {
            payload: Payload::inline("image/png", mask)?,
            label: None,
        }),
        Event::Output(segment::found_output(&found)?),
    ])
}

/// Reads `png`, checks its size against `size`, reduces it to one channel and says whether any
/// pixel is selected (above half). A mask with nothing selected comes back all black.
fn finish(png: &[u8], size: &Size) -> Result<(Vec<u8>, bool), AiError> {
    let mut decoder =
        png::Decoder::new_with_limits(Cursor::new(png), png::Limits { bytes: MAX_ALLOC });
    decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|_| bad_mask())?;
    let (width, height) = (reader.info().width, reader.info().height);
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(bad_mask());
    }
    if !size.accepts(width, height) {
        return Err(AiError::new(
            ErrorKind::Provider,
            "The workflow's mask is not the size of the picture, so it was not used.",
        ));
    }
    let mut buffer = vec![0u8; reader.output_buffer_size().ok_or_else(bad_mask)?];
    let frame = reader.next_frame(&mut buffer).map_err(|_| bad_mask())?;
    let channels = match frame.color_type {
        ColorType::Grayscale => 1,
        ColorType::GrayscaleAlpha => 2,
        ColorType::Rgb => 3,
        ColorType::Rgba => 4,
        ColorType::Indexed => return Err(bad_mask()),
    };
    let pixels = buffer.get(..frame.buffer_size()).ok_or_else(bad_mask)?;
    let count = width as usize * height as usize;
    if frame.bit_depth != BitDepth::Eight || pixels.len() < count * channels {
        return Err(bad_mask());
    }
    // The first channel is the mask: a graph turns its mask into a picture with equal channels.
    let mut gray: Vec<u8> = pixels
        .chunks_exact(channels)
        .take(count)
        .map(|p| p.first().copied().unwrap_or(0))
        .collect();
    let matched = gray.iter().any(|v| *v > 127);
    if !matched {
        gray.iter_mut().for_each(|v| *v = 0);
    }
    Ok((encode_gray(&gray, width, height)?, matched))
}

/// An 8-bit grayscale PNG of `gray`, which holds `width * height` samples.
fn encode_gray(gray: &[u8], width: u32, height: u32) -> Result<Vec<u8>, AiError> {
    /// Turns any encoder error into an internal "could not be saved" error.
    fn fail<E>(_: E) -> AiError {
        AiError::new(ErrorKind::Internal, "The mask could not be saved.")
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

#[cfg(test)]
mod tests;
