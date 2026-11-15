//! A returned picture is decoded in full before an adapter calls its job a success.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use craft_ai_media::decoded_result;
use image::{ImageFormat, RgbImage};
use std::io::Cursor;

/// A `width` × `height` picture with varied pixels, encoded as `format`.
fn encoded(format: ImageFormat, width: u32, height: u32) -> Vec<u8> {
    let picture = RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x * 7) as u8, (y * 13) as u8, ((x + y) * 3) as u8])
    });
    let mut out = Cursor::new(Vec::new());
    picture.write_to(&mut out, format).unwrap();
    out.into_inner()
}

/// Whole PNG, JPEG and WebP results are accepted with their media type.
#[test]
fn whole_results_are_accepted() {
    for (format, media) in [
        (ImageFormat::Png, "image/png"),
        (ImageFormat::Jpeg, "image/jpeg"),
        (ImageFormat::WebP, "image/webp"),
    ] {
        assert_eq!(
            decoded_result(&encoded(format, 64, 48)),
            Some(media),
            "{media}"
        );
    }
}

/// A result whose header is intact but whose picture data is cut short is refused.
#[test]
fn a_result_cut_short_is_refused() {
    for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP] {
        let whole = encoded(format, 64, 48);
        let cut = &whole[..whole.len() / 2];
        assert!(
            craft_ai_media::image_size(cut).is_some(),
            "{format:?}: the header must still read, or this test proves nothing"
        );
        assert_eq!(decoded_result(cut), None, "{format:?}");
    }
}

/// A picture that is not one of the three formats is refused.
#[test]
fn other_bytes_are_refused() {
    assert_eq!(decoded_result(b"GIF89a not a result"), None);
    assert_eq!(decoded_result(&[]), None);
}
