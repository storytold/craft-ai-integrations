//! A picture's width and height from its header, for PNG, JPEG and WebP, without decoding pixels.
//!
//! The inpainting adapters use it to check a source against their size limits and against the
//! mask before anything is sent. It reads only the bytes it needs and never allocates for pixels,
//! so a hostile header cannot make it use memory. `None` means the header could not be read; the
//! caller refuses the picture.

/// The size of a PNG, JPEG or WebP picture from its header, or `None` when it cannot be read.
pub fn image_size(data: &[u8]) -> Option<(u32, u32)> {
    match crate::sniff::media_type(data)?.0 {
        "image/png" => crate::mask::png_size(data),
        "image/jpeg" => jpeg_size(data),
        "image/webp" => webp_size(data),
        _ => None,
    }
    .filter(|&(w, h)| w > 0 && h > 0)
}

/// A big-endian 16-bit number at `at`.
fn be16(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_be_bytes([
        *data.get(at)?,
        *data.get(at + 1)?,
    ])))
}

/// A little-endian number of `len` bytes (at most 4) at `at`.
fn le(data: &[u8], at: usize, len: usize) -> Option<u32> {
    let bytes = data.get(at..at + len)?;
    Some(bytes.iter().rev().fold(0, |n, &b| (n << 8) | u32::from(b)))
}

/// Walks the JPEG markers to the first start-of-frame and reads its size. Stops at the first
/// scan, because a frame header always comes before the picture data.
fn jpeg_size(data: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2; // past FF D8
    loop {
        if *data.get(at)? != 0xFF {
            return None;
        }
        // Any number of FF bytes may pad before a marker.
        while *data.get(at)? == 0xFF {
            at += 1;
        }
        let marker = *data.get(at)?;
        at += 1;
        match marker {
            // Markers with no length: restart markers, TEM and a stray start-of-image.
            0x01 | 0xD0..=0xD8 => continue,
            // Start of scan or end of image before any frame: no size to find.
            0xD9 | 0xDA => return None,
            // Start-of-frame markers, all but DHT (C4), JPG (C8) and DAC (CC).
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                // Length (2), precision (1), height (2), width (2).
                let height = be16(data, at + 3)?;
                let width = be16(data, at + 5)?;
                return Some((width, height));
            }
            _ => {
                let length = usize::try_from(be16(data, at)?).ok()?;
                if length < 2 {
                    return None;
                }
                at += length;
            }
        }
    }
}

/// Reads the size from the first chunk of a WebP file: lossy (`VP8 `), lossless (`VP8L`) or
/// extended (`VP8X`, which states the canvas size).
fn webp_size(data: &[u8]) -> Option<(u32, u32)> {
    let chunk = data.get(12..16)?;
    let body = 20;
    match chunk {
        b"VP8X" => Some((le(data, body + 4, 3)? + 1, le(data, body + 7, 3)? + 1)),
        b"VP8 " => {
            // A key frame starts with a three-byte tag, then the start code 9D 01 2A.
            if data.get(body + 3..body + 6)? != [0x9D, 0x01, 0x2A] {
                return None;
            }
            Some((
                le(data, body + 6, 2)? & 0x3FFF,
                le(data, body + 8, 2)? & 0x3FFF,
            ))
        }
        b"VP8L" => {
            if *data.get(body)? != 0x2F {
                return None;
            }
            let bits = le(data, body + 1, 4)?;
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal JPEG header: SOI, an APP0 segment to skip, padding, then SOF0 for 640 × 480.
    fn jpeg(width: u16, height: u16, frame_marker: u8) -> Vec<u8> {
        let mut d = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        d.extend_from_slice(&[0xFF, 0xFF, frame_marker, 0x00, 0x11, 0x08]);
        d.extend_from_slice(&height.to_be_bytes());
        d.extend_from_slice(&width.to_be_bytes());
        d.extend_from_slice(&[0x03; 12]);
        d
    }

    /// A RIFF/WEBP header around one chunk of `fourcc` with `body`.
    fn webp(fourcc: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut d = b"RIFF\0\0\0\0WEBP".to_vec();
        d.extend_from_slice(fourcc);
        d.extend_from_slice(&(body.len() as u32).to_le_bytes());
        d.extend_from_slice(body);
        d
    }

    /// Each format reads the size its header states, and a progressive JPEG frame counts too.
    #[test]
    fn reads_jpeg_and_every_webp_kind() {
        assert_eq!(image_size(&jpeg(640, 480, 0xC0)), Some((640, 480)));
        assert_eq!(image_size(&jpeg(33, 7, 0xC2)), Some((33, 7)));
        // Lossy: tag, start code, 14-bit width and height.
        let lossy = [0, 0, 0, 0x9D, 0x01, 0x2A, 0x80, 0x02, 0xE0, 0x01];
        assert_eq!(image_size(&webp(b"VP8 ", &lossy)), Some((640, 480)));
        // Lossless: signature, then width - 1 and height - 1 packed in 14 bits each.
        let bits: u32 = 639 | (479 << 14);
        let mut lossless = vec![0x2F];
        lossless.extend_from_slice(&bits.to_le_bytes());
        assert_eq!(image_size(&webp(b"VP8L", &lossless)), Some((640, 480)));
        // Extended: flags, then canvas width - 1 and height - 1 in 24 bits each.
        let extended = [0, 0, 0, 0, 0x7F, 0x02, 0, 0xDF, 0x01, 0];
        assert_eq!(image_size(&webp(b"VP8X", &extended)), Some((640, 480)));
    }

    /// Headers that end early, lie about a length, or hide the frame behind a scan are refused,
    /// and so is a zero size.
    #[test]
    fn unreadable_headers_are_refused() {
        let whole = jpeg(640, 480, 0xC0);
        assert_eq!(image_size(&whole[..12]), None);
        assert_eq!(image_size(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x01]), None);
        assert_eq!(image_size(&[0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x08]), None);
        assert_eq!(image_size(&jpeg(0, 480, 0xC0)), None);
        // DHT is not a frame header, so its bytes are not read as a size.
        assert_eq!(image_size(&jpeg(640, 480, 0xC4)), None);
        assert_eq!(
            image_size(&webp(b"VP8 ", &[0, 0, 0, 1, 2, 3, 4, 5, 6, 7])),
            None
        );
        assert_eq!(image_size(&webp(b"VP8L", &[0x2E, 0, 0, 0, 0])), None);
        assert_eq!(image_size(b"not a picture"), None);
    }
}
