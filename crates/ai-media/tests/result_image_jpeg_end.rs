//! A JPEG result is whole when its markers reach the end-of-image marker, whatever follows it, and
//! an end marker inside a metadata segment does not make a cut-short picture look whole.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use craft_ai_media::decoded_result;
use image::{ImageFormat, RgbImage};
use std::io::Cursor;

/// A 64 × 48 JPEG with varied pixels.
fn jpeg() -> Vec<u8> {
    let picture = RgbImage::from_fn(64, 48, |x, y| {
        image::Rgb([(x * 7) as u8, (y * 13) as u8, ((x + y) * 3) as u8])
    });
    let mut out = Cursor::new(Vec::new());
    picture.write_to(&mut out, ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

/// `whole` with an APP1 segment, holding a bare end-of-image marker, inserted after its start.
fn with_end_marker_in_metadata(whole: &[u8]) -> Vec<u8> {
    let mut out = whole[..2].to_vec();
    out.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x06, 0xFF, 0xD9, 0xFF, 0xD9]);
    out.extend_from_slice(&whole[2..]);
    out
}

/// Bytes after the end marker, zero or not, do not make a whole picture unreadable.
#[test]
fn trailing_bytes_after_the_end_marker_are_allowed() {
    for trailer in [&[0u8; 4][..], b"trailer", &[0xFF, 0x12, 0x34]] {
        let mut data = jpeg();
        data.extend_from_slice(trailer);
        assert_eq!(decoded_result(&data), Some("image/jpeg"), "{trailer:?}");
    }
}

/// An end marker inside a metadata segment is skipped with the segment: the picture is whole only
/// when its own end marker is reached.
#[test]
fn an_end_marker_inside_metadata_does_not_count() {
    let whole = with_end_marker_in_metadata(&jpeg());
    assert_eq!(decoded_result(&whole), Some("image/jpeg"));
    let cut = &whole[..whole.len() - 40];
    assert!(craft_ai_media::image_size(cut).is_some());
    assert_eq!(decoded_result(cut), None);
}
