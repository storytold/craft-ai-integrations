//! A returned picture, fully decoded once before it is handed to an app.
//!
//! The fill adapters used to check a result by its first bytes and its size only. A picture with a
//! good signature and a broken body then finished as a successful job and failed later, when the
//! app imported it. This decodes every pixel of a PNG, JPEG or WebP result, so a broken one fails
//! the job instead.
//!
//! Invariants: the size is read from the header and checked against [`MAX_RESULT_PIXELS`] before
//! any pixels are decoded, and the decoder's own allocation limit is set from it, so a hostile
//! header cannot make it allocate more. A PNG is read one row at a time. JPEG and WebP use the
//! `image` crate, which the workspace already uses for the local engines. Its JPEG decoder fills in
//! a picture cut short rather than failing, so a JPEG must also end with its end-of-image marker.

use image::{ImageFormat, ImageReader, Limits};
use std::io::Cursor;

/// The largest result decoded: 8192 × 8192, above any size these services return (4K at most).
pub const MAX_RESULT_PIXELS: u64 = 8192 * 8192;

/// The media type of a returned picture when it is a PNG, JPEG or WebP that decodes in full within
/// [`MAX_RESULT_PIXELS`], or `None`. The caller fails the job on `None`.
pub fn decoded_result(data: &[u8]) -> Option<&'static str> {
    let (media, _) = crate::sniff::media_type(data)?;
    let (width, height) = crate::image_size(data)?;
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_RESULT_PIXELS {
        return None;
    }
    let readable = match media {
        "image/png" => png_rows_read(data, height),
        "image/jpeg" => ends_with_eoi(data) && decodes(data, ImageFormat::Jpeg, width, height),
        "image/webp" => decodes(data, ImageFormat::WebP, width, height),
        _ => false,
    };
    readable.then_some(media)
}

/// Whether every row of a PNG decodes, holding one row at a time.
fn png_rows_read(data: &[u8], height: u32) -> bool {
    let limit = png::Limits {
        bytes: usize::try_from(MAX_RESULT_PIXELS * 8).unwrap_or(usize::MAX),
    };
    let Ok(mut reader) = png::Decoder::new_with_limits(Cursor::new(data), limit).read_info() else {
        return false;
    };
    let mut rows = 0u32;
    loop {
        match reader.next_row() {
            Ok(Some(_)) => rows = rows.saturating_add(1),
            Ok(None) => break,
            Err(_) => return false,
        }
    }
    // An interlaced picture yields each row once per pass, so only a shortfall is an error.
    rows >= height
}

/// Whether a JPEG ends with its end-of-image marker (`FF D9`), allowing zero padding after it.
/// The JPEG decoder fills in a picture cut short instead of failing, so this is what catches one.
fn ends_with_eoi(data: &[u8]) -> bool {
    let end = data.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    data.get(..end).is_some_and(|d| d.ends_with(&[0xFF, 0xD9]))
}

/// Whether a JPEG or WebP decodes in full, with the decoder's allocation capped for its size.
fn decodes(data: &[u8], format: ImageFormat, width: u32, height: u32) -> bool {
    let mut limits = Limits::default();
    limits.max_image_width = Some(width);
    limits.max_image_height = Some(height);
    // Four bytes a pixel covers every layout these formats decode to.
    limits.max_alloc = Some(u64::from(width) * u64::from(height) * 4);
    let mut reader = ImageReader::with_format(Cursor::new(data), format);
    reader.limits(limits);
    reader.decode().is_ok()
}
