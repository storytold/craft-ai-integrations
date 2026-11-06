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
//! `image` crate, which the workspace already uses for the local engines. Their decodes share one
//! process-wide memory budget ([`DECODE_BUDGET`]), so many workers finishing at once wait their
//! turn instead of each holding a full frame. The JPEG decoder fills in a picture cut short rather
//! than failing, so a JPEG's marker structure must also reach its end-of-image marker.

use image::{ImageFormat, ImageReader, Limits};
use std::io::Cursor;
use std::sync::{Condvar, Mutex, PoisonError};

/// The largest result decoded: 8192 × 8192, above any size these services return (4K at most).
pub const MAX_RESULT_PIXELS: u64 = 8192 * 8192;

/// Bytes all JPEG and WebP decodes in the process may hold at once: two decodes of the largest
/// result. A Gemini fill can run 32 workers, and without this each could hold 256 MiB at once.
const DECODE_BUDGET: u64 = 512 * 1024 * 1024;

/// Four bytes a pixel covers every layout these formats decode to.
const BYTES_PER_PIXEL: u64 = 4;

// One decode of the largest result must fit the budget, or its reservation would wait for ever.
const _: () = assert!(MAX_RESULT_PIXELS * BYTES_PER_PIXEL <= DECODE_BUDGET);

/// Bytes currently reserved by running decodes, and the signal that some were given back.
static RESERVED: Mutex<u64> = Mutex::new(0);
static RELEASED: Condvar = Condvar::new();

/// A share of [`DECODE_BUDGET`], given back when dropped.
pub(crate) struct Reservation(u64);

impl Reservation {
    /// Waits until `bytes` fit within the budget beside the decodes already running, then holds
    /// them. `bytes` must not exceed [`DECODE_BUDGET`]; the callers' pixel bound guarantees it.
    pub(crate) fn take(bytes: u64) -> Self {
        let mut reserved = RESERVED.lock().unwrap_or_else(PoisonError::into_inner);
        while reserved.saturating_add(bytes) > DECODE_BUDGET {
            reserved = RELEASED
                .wait(reserved)
                .unwrap_or_else(PoisonError::into_inner);
        }
        *reserved += bytes;
        Self(bytes)
    }
}

impl Drop for Reservation {
    /// Gives the bytes back and wakes every waiting decode, since any of them may now fit.
    fn drop(&mut self) {
        let mut reserved = RESERVED.lock().unwrap_or_else(PoisonError::into_inner);
        *reserved = reserved.saturating_sub(self.0);
        RELEASED.notify_all();
    }
}

/// The media type of a returned picture when it is a PNG, JPEG or WebP that decodes in full within
/// [`MAX_RESULT_PIXELS`], or `None`. The caller fails the job on `None`. May wait for other
/// decodes to finish first (see [`DECODE_BUDGET`]).
pub fn decoded_result(data: &[u8]) -> Option<&'static str> {
    let (media, _) = crate::sniff::media_type(data)?;
    let (width, height) = crate::image_size(data)?;
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_RESULT_PIXELS {
        return None;
    }
    let readable = match media {
        "image/png" => png_rows_read(data, height),
        "image/jpeg" => reaches_eoi(data) && decodes(data, ImageFormat::Jpeg, width, height),
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

/// Whether a JPEG's markers lead from its start to its end-of-image marker (`FF D9`). Segments
/// with a length are skipped whole, so the markers of an embedded EXIF thumbnail are never taken
/// for the picture's own. Scan data is read up to the next real marker, past stuffed `FF 00` bytes
/// and restart markers. Bytes after the end marker are ignored. The JPEG decoder fills in a
/// picture cut short instead of failing, so this is what catches one.
fn reaches_eoi(data: &[u8]) -> bool {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return false;
    }
    let mut at = 2usize;
    loop {
        if data.get(at) != Some(&0xFF) {
            return false;
        }
        // Any number of FF bytes may pad before a marker.
        while data.get(at) == Some(&0xFF) {
            at += 1;
        }
        let Some(&marker) = data.get(at) else {
            return false;
        };
        at += 1;
        match marker {
            0xD9 => return true,
            // Markers with no length: restart markers, TEM and a stray start-of-image.
            0xD0..=0xD8 | 0x01 => {}
            _ => {
                let Some(length) = data
                    .get(at..at.saturating_add(2))
                    .and_then(|b| <[u8; 2]>::try_from(b).ok())
                    .map(|b| usize::from(u16::from_be_bytes(b)))
                    .filter(|&n| n >= 2)
                else {
                    return false;
                };
                at = at.saturating_add(length);
                if marker == 0xDA {
                    match scan_end(data, at) {
                        Some(next) => at = next,
                        None => return false,
                    }
                }
            }
        }
    }
}

/// Where the entropy-coded data starting at `at` ends: the position of the next marker that is
/// not a stuffed zero (`FF 00`) or a restart marker, or `None` when the data runs out first.
fn scan_end(data: &[u8], mut at: usize) -> Option<usize> {
    loop {
        at += data.get(at..)?.iter().position(|&b| b == 0xFF)?;
        match *data.get(at + 1)? {
            0x00 | 0xD0..=0xD7 => at += 2,
            // A fill byte: the marker, if any, starts at the next FF.
            0xFF => at += 1,
            _ => return Some(at),
        }
    }
}

/// Whether a JPEG or WebP decodes in full, with the decoder's allocation capped for its size and
/// that much reserved from the shared [`DECODE_BUDGET`] for the length of the decode.
fn decodes(data: &[u8], format: ImageFormat, width: u32, height: u32) -> bool {
    let bytes = u64::from(width) * u64::from(height) * BYTES_PER_PIXEL;
    let mut limits = Limits::default();
    limits.max_image_width = Some(width);
    limits.max_image_height = Some(height);
    limits.max_alloc = Some(bytes);
    let mut reader = ImageReader::with_format(Cursor::new(data), format);
    reader.limits(limits);
    let _held = Reservation::take(bytes);
    reader.decode().is_ok()
}

#[cfg(test)]
#[path = "result_image_tests.rs"]
mod tests;
