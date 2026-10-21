use jpeg_encoder::{ColorType as JpegColor, Encoder};
use png::{BitDepth, ColorType};

use super::{Rgb, decode_jpeg, decode_picture, decode_png, encode_mask};

/// A PNG of the given colour type, 8 bits a channel, holding `data` as `w` by `h` pixels.
fn png_of(color: ColorType, w: u32, h: u32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, w, h);
    e.set_color(color);
    e.set_depth(BitDepth::Eight);
    let mut w = e.write_header().unwrap();
    w.write_image_data(data).unwrap();
    w.finish().unwrap();
    out
}

/// Left half red, right half blue.
fn two_halves(width: u16, height: u16) -> Vec<u8> {
    let mut pixels = Vec::new();
    for _ in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(if x < width / 2 {
                &[255, 0, 0]
            } else {
                &[0, 0, 255]
            });
        }
    }
    pixels
}

/// A JPEG of the given pixels, with an EXIF orientation block when one is given.
fn jpeg_of(pixels: &[u8], w: u16, h: u16, color: JpegColor, exif: Option<u16>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = Encoder::new(&mut out, 100);
    if let Some(orientation) = exif {
        encoder.add_app_segment(1, exif_block(orientation)).unwrap();
    }
    encoder.encode(pixels, w, h, color).unwrap();
    out
}

/// `Exif\0\0`, a little-endian TIFF header, and one entry: Orientation (tag 0x0112).
fn exif_block(orientation: u16) -> Vec<u8> {
    let mut b = b"Exif\0\0".to_vec();
    b.extend_from_slice(&[0x49, 0x49, 0x2A, 0x00, 8, 0, 0, 0]);
    b.extend_from_slice(&[1, 0]);
    b.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0]);
    b.extend_from_slice(&orientation.to_le_bytes());
    b.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    b
}

/// True when every channel of `got` is within 40 of `want`, which allows for JPEG loss.
fn near(got: &[u8], want: [u8; 3]) -> bool {
    got.iter().zip(want).all(|(g, w)| g.abs_diff(w) < 40)
}

/// The three bytes of the pixel at (`x`, `y`).
fn px(rgb: &Rgb, x: usize, y: usize) -> &[u8] {
    let at = (y * rgb.width + x) * 3;
    &rgb.bytes[at..at + 3]
}

/// RGB, RGBA, grey and grey with alpha all come back as three channels, with the alpha dropped and
/// the grey repeated.
#[test]
fn every_png_colour_type_becomes_rgb() {
    let rgb = decode_png(&png_of(ColorType::Rgb, 2, 1, &[1, 2, 3, 4, 5, 6])).unwrap();
    assert_eq!(
        (rgb.width, rgb.height, rgb.bytes),
        (2, 1, vec![1, 2, 3, 4, 5, 6])
    );
    let rgba = decode_png(&png_of(ColorType::Rgba, 2, 1, &[9, 8, 7, 0, 6, 5, 4, 255])).unwrap();
    assert_eq!(rgba.bytes, vec![9, 8, 7, 6, 5, 4]);
    let gray = decode_png(&png_of(ColorType::Grayscale, 2, 1, &[5, 6])).unwrap();
    assert_eq!(gray.bytes, vec![5, 5, 5, 6, 6, 6]);
    let ga = decode_png(&png_of(ColorType::GrayscaleAlpha, 1, 1, &[5, 255])).unwrap();
    assert_eq!(ga.bytes, vec![5, 5, 5]);
}

/// Text, nothing and a GIF header are refused with a message that names PNG and JPEG.
#[test]
fn something_that_is_not_a_picture_is_refused_in_plain_words() {
    for bytes in [&b"not a picture"[..], b"", b"GIF89a"] {
        let e = decode_picture(bytes).err().unwrap();
        assert!(e.message.contains("PNG or a JPEG"), "{}", e.message);
    }
}

/// A PNG and a JPEG decode by their first bytes, and a bare JPEG start is a damaged JPEG, not "not
/// a picture".
#[test]
fn a_picture_is_told_by_its_first_bytes_not_by_what_the_app_called_it() {
    let png = png_of(ColorType::Rgb, 1, 1, &[1, 2, 3]);
    assert_eq!(decode_picture(&png).unwrap().bytes, vec![1, 2, 3]);
    let jpeg = jpeg_of(&two_halves(16, 8), 16, 8, JpegColor::Rgb, None);
    assert_eq!(decode_picture(&jpeg).unwrap().width, 16);
    // A JPEG start with nothing behind it is a damaged JPEG, not "not a picture".
    let e = decode_picture(&[0xFF, 0xD8, 0xFF, 0xE0]).err().unwrap();
    assert!(e.message.contains("could not be read"));
}

/// A mask encodes at the size given and decodes to the same coverage, with values over one held at
/// white.
#[test]
fn a_mask_round_trips_at_the_size_of_the_source_with_coverage_clamped() {
    let png = encode_mask(&[0.0, 1.0, 0.5, 2.0], 2, 2).unwrap();
    let back = decode_png(&png).unwrap();
    assert_eq!((back.width, back.height), (2, 2));
    let first_channel: Vec<u8> = back.bytes.iter().step_by(3).copied().collect();
    assert_eq!(first_channel, vec![0, 255, 128, 255]);
}

/// A mask that is not square keeps its width and height the right way round, and each row keeps
/// its own values.
#[test]
fn the_mask_comes_back_with_width_and_height_the_right_way_round() {
    let png = encode_mask(&[0.0, 1.0, 0.0, 1.0, 0.0, 1.0], 3, 2).unwrap();
    let back = decode_png(&png).unwrap();
    assert_eq!((back.width, back.height), (3, 2));
    let first_channel: Vec<u8> = back.bytes.iter().step_by(3).copied().collect();
    assert_eq!(first_channel, vec![0, 255, 0, 255, 0, 255]);
}

/// A colour JPEG keeps its size and its left and right colours.
#[test]
fn a_colour_jpeg_becomes_rgb_at_its_own_size() {
    let jpeg = jpeg_of(&two_halves(16, 8), 16, 8, JpegColor::Rgb, None);
    let rgb = decode_jpeg(&jpeg).unwrap();
    assert_eq!(
        (rgb.width, rgb.height, rgb.bytes.len()),
        (16, 8, 16 * 8 * 3)
    );
    assert!(near(px(&rgb, 1, 4), [255, 0, 0]));
    assert!(near(px(&rgb, 14, 4), [0, 0, 255]));
}

/// A greyscale JPEG comes back with red, green and blue equal.
#[test]
fn a_greyscale_jpeg_becomes_three_equal_channels() {
    let jpeg = jpeg_of(&[90; 8 * 8], 8, 8, JpegColor::Luma, None);
    let rgb = decode_jpeg(&jpeg).unwrap();
    let p = px(&rgb, 3, 3);
    assert!(
        p[0].abs_diff(90) < 6 && p[0] == p[1] && p[1] == p[2],
        "{p:?}"
    );
}

/// A JPEG marked as turned a quarter comes back with its sides swapped and its colours on the right
/// ends.
#[test]
fn the_exif_turn_is_applied_so_the_picture_is_upright() {
    // Orientation 6: stored 16x8 (red left, blue right) becomes 8x16, red on top, blue below.
    let jpeg = jpeg_of(&two_halves(16, 8), 16, 8, JpegColor::Rgb, Some(6));
    let rgb = decode_jpeg(&jpeg).unwrap();
    assert_eq!((rgb.width, rgb.height), (8, 16));
    assert!(near(px(&rgb, 4, 1), [255, 0, 0]));
    assert!(near(px(&rgb, 4, 14), [0, 0, 255]));
}

/// No orientation, orientation one and an unknown number leave the stored size as it was.
#[test]
fn orientation_one_no_exif_and_nonsense_leave_the_picture_alone() {
    for exif in [None, Some(1), Some(99)] {
        let jpeg = jpeg_of(&two_halves(16, 8), 16, 8, JpegColor::Rgb, exif);
        let rgb = decode_jpeg(&jpeg).unwrap();
        assert_eq!((rgb.width, rgb.height), (16, 8), "{exif:?}");
    }
}

/// A JPEG header that claims 30,000 by 30,000 pixels is refused as too large without decoding the
/// rest.
#[test]
fn a_huge_picture_is_refused_from_its_header_before_any_pixel_is_decoded() {
    let mut jpeg = jpeg_of(&two_halves(16, 8), 16, 8, JpegColor::Rgb, None);
    // The start-of-frame marker (FF C0) is followed by length, precision, height, width.
    let at = jpeg.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap() + 5;
    jpeg[at..at + 2].copy_from_slice(&30_000u16.to_be_bytes());
    jpeg[at + 2..at + 4].copy_from_slice(&30_000u16.to_be_bytes());
    let e = decode_jpeg(&jpeg).err().unwrap();
    assert!(e.message.contains("too large"), "{}", e.message);
}

/// Cut-off and corrupted JPEGs give a plain error or a picture, and never a panic.
#[test]
fn damaged_and_cut_off_files_are_plain_errors_not_panics() {
    let jpeg = jpeg_of(&two_halves(64, 64), 64, 64, JpegColor::Rgb, None);
    for cut in [2, 20, jpeg.len() / 2] {
        if let Err(e) = decode_jpeg(&jpeg[..cut]) {
            assert!(e.message.contains("could not be read"), "{}", e.message);
        }
    }
    let mut noisy = jpeg.clone();
    for byte in noisy.iter_mut().skip(40).step_by(3) {
        *byte ^= 0x5A;
    }
    let _ = decode_jpeg(&noisy); // any answer but a panic
}

/// The CRC-32 a PNG chunk ends with.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A PNG header that claims 20,000 by 20,000 pixels is refused as too large before any pixel is
/// read. (A header with a zero side never reaches that check: the decoder refuses it first.)
#[test]
fn a_huge_png_is_refused_from_its_header() {
    for (w, h, message) in [
        (20_000u32, 20_000u32, "too large"),
        (100_000, 2_000, "too large"),
        // Few pixels, but a side no decoder here accepts.
        (70_000, 1, "too large"),
        (1, 70_000, "too large"),
    ] {
        let mut png = png_of(ColorType::Rgb, 2, 2, &[0; 12]);
        // The header chunk: width, height, then its checksum over the type and the data.
        png[16..20].copy_from_slice(&w.to_be_bytes());
        png[20..24].copy_from_slice(&h.to_be_bytes());
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        let e = decode_png(&png).err().unwrap();
        assert!(e.message.contains(message), "{w}x{h}: {}", e.message);
    }
}
