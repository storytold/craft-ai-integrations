//! Pictures in and masks out: a PNG or JPEG source becomes the RGB bytes the model reads, and the
//! mask goes back as a grayscale PNG at the size of the upright source.
//!
//! A JPEG's EXIF turn is applied, so a phone photo taken sideways comes back upright and a click
//! normalised to the picture lands where the person clicked. A PNG is taken as stored. Hostile
//! input is bounded: the size is read from the header and refused before any pixel is decoded,
//! and both decoders get an allocation cap.

use std::io::Cursor;

use craft_ai_core::{AiError, ErrorKind};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, metadata::Orientation};
use png::{BitDepth, ColorType, Transformations};

/// Pictures larger than this are refused before any pixel is read.
pub const MAX_PIXELS: u64 = 100_000_000;

/// The most memory either decoder may ask for. A 100 megapixel picture is 300 MB of RGB.
const MAX_ALLOC: usize = 900 * 1024 * 1024;

/// What a person is told when a picture cannot be read at all.
pub const READ_FAILED: &str = "The picture could not be read. Use a PNG or a JPEG.";

const PNG_START: [u8; 4] = [0x89, b'P', b'N', b'G'];
const JPEG_START: [u8; 3] = [0xFF, 0xD8, 0xFF];

/// An 8-bit RGB picture, row by row.
pub struct Rgb {
    pub width: usize,
    pub height: usize,
    pub bytes: Vec<u8>,
}

/// The refusal for a picture over [`MAX_PIXELS`], in plain words.
fn too_large() -> AiError {
    AiError::invalid("The picture is too large to select from (over 100 megapixels).")
}

/// Turns any decoder error into the one plain "could not be read" message, so no library detail
/// reaches the person.
fn unreadable<E>(_: E) -> AiError {
    AiError::invalid(READ_FAILED)
}

/// Decodes a PNG or a JPEG, told apart by their first bytes rather than by the media type the app
/// sent, which is often only a guess from a file name.
pub fn decode_picture(data: &[u8]) -> Result<Rgb, AiError> {
    if data.starts_with(&PNG_START) {
        decode_png(data)
    } else if data.starts_with(&JPEG_START) {
        decode_jpeg(data)
    } else {
        Err(AiError::invalid(READ_FAILED))
    }
}

/// Decodes a PNG of any colour type to RGB. Transparency is dropped.
fn decode_png(data: &[u8]) -> Result<Rgb, AiError> {
    let mut decoder =
        png::Decoder::new_with_limits(Cursor::new(data), png::Limits { bytes: MAX_ALLOC });
    decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(unreadable)?;
    let info = reader.info();
    let (w, h) = (u64::from(info.width), u64::from(info.height));
    if w == 0 || h == 0 || w * h > MAX_PIXELS {
        return Err(too_large());
    }
    let mut buffer = vec![0u8; reader.output_buffer_size().ok_or_else(too_large)?];
    let frame = reader.next_frame(&mut buffer).map_err(unreadable)?;
    let (width, height) = (frame.width as usize, frame.height as usize);
    let pixels = buffer.get(..frame.buffer_size()).unwrap_or(&[]);
    let channels = match frame.color_type {
        ColorType::Rgb => 3,
        ColorType::Rgba => 4,
        ColorType::Grayscale => 1,
        ColorType::GrayscaleAlpha => 2,
        ColorType::Indexed => return Err(AiError::invalid(READ_FAILED)),
    };
    if frame.bit_depth != BitDepth::Eight || pixels.len() < width * height * channels {
        return Err(AiError::invalid(READ_FAILED));
    }
    let mut bytes = Vec::with_capacity(width * height * 3);
    for px in pixels.chunks_exact(channels).take(width * height) {
        match px {
            [g] | [g, _] => bytes.extend_from_slice(&[*g, *g, *g]),
            [r, g, b, ..] => bytes.extend_from_slice(&[*r, *g, *b]),
            _ => return Err(AiError::invalid(READ_FAILED)),
        }
    }
    Ok(Rgb {
        width,
        height,
        bytes,
    })
}

/// Decodes a JPEG (baseline, progressive, grey or CMYK) and turns it upright.
fn decode_jpeg(data: &[u8]) -> Result<Rgb, AiError> {
    let mut reader = ImageReader::with_format(Cursor::new(data), ImageFormat::Jpeg);
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_ALLOC as u64);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(unreadable)?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(too_large());
    }
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut picture = DynamicImage::from_decoder(decoder).map_err(unreadable)?;
    picture.apply_orientation(orientation);
    let rgb = picture.into_rgb8();
    Ok(Rgb {
        width: rgb.width() as usize,
        height: rgb.height() as usize,
        bytes: rgb.into_raw(),
    })
}

/// Coverage 0..1 per pixel as an 8-bit grayscale PNG (white is selected).
pub fn encode_mask(alpha: &[f32], width: usize, height: usize) -> Result<Vec<u8>, AiError> {
    /// Turns any encoder or size-conversion error into an internal "could not be saved" error.
    fn fail<E>(_: E) -> AiError {
        AiError::new(ErrorKind::Internal, "The mask could not be saved.")
    }
    let gray: Vec<u8> = alpha
        .iter()
        .map(|a| (a.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(
        &mut out,
        u32::try_from(width).map_err(fail)?,
        u32::try_from(height).map_err(fail)?,
    );
    encoder.set_color(ColorType::Grayscale);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(fail)?;
    writer.write_image_data(&gray).map_err(fail)?;
    writer.finish().map_err(fail)?;
    Ok(out)
}

#[cfg(test)]
mod tests;
