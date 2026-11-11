//! `mask::binary` reads the mask a row at a time. These cases pin the rules that depend on the
//! whole picture, or on the row count, so the streaming read cannot change them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use craft_ai_media::mask::{Bounds, binary};
use png::{BitDepth, ColorType};

const BOUNDS: Bounds = Bounds {
    service: "the test service",
    max_side: 64,
    max_pixels: 64 * 64,
};

/// A PNG of `width` × `height` whose pixel at (x, y) is `px(x, y)`.
fn png_of(
    color: ColorType,
    depth: BitDepth,
    width: u32,
    height: u32,
    px: impl Fn(u32, u32) -> Vec<u8>,
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(color);
    encoder.set_depth(depth);
    let mut writer = encoder.write_header().unwrap();
    let pixels: Vec<u8> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .flat_map(|(x, y)| px(x, y))
        .collect();
    writer.write_image_data(&pixels).unwrap();
    writer.finish().unwrap();
    out
}

/// The grey samples of the binary mask.
fn samples(png: &[u8]) -> Vec<u8> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .unwrap();
    let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut buffer).unwrap();
    assert_eq!(frame.color_type, ColorType::Grayscale);
    buffer.truncate(frame.buffer_size());
    buffer
}

/// One translucent pixel on the LAST row switches every earlier row to the alpha reading.
#[test]
fn a_late_translucent_pixel_changes_how_earlier_rows_read() {
    // White and opaque everywhere (paint by brightness), except one clear pixel at the end.
    let mask = png_of(ColorType::Rgba, BitDepth::Eight, 2, 3, |x, y| {
        if (x, y) == (1, 2) {
            vec![0, 0, 0, 0]
        } else {
            vec![255, 255, 255, 255]
        }
    });
    let (out, w, h) = binary(&mask, BOUNDS).unwrap();
    assert_eq!((w, h), (2, 3));
    // Read by alpha: only the clear pixel paints, the white ones are kept.
    assert_eq!(samples(&out), vec![0, 0, 0, 0, 0, 255]);
}

/// Without any translucent pixel, the same picture reads by brightness.
#[test]
fn an_opaque_mask_reads_by_brightness_on_every_row() {
    let mask = png_of(ColorType::Rgba, BitDepth::Eight, 2, 3, |x, _| {
        if x == 0 {
            vec![255, 255, 255, 255]
        } else {
            vec![0, 0, 0, 255]
        }
    });
    let (out, ..) = binary(&mask, BOUNDS).unwrap();
    assert_eq!(samples(&out), vec![255, 0, 255, 0, 255, 0]);
}

/// A 16-bit mask is stripped to 8 bits and read the same way.
#[test]
fn a_sixteen_bit_mask_is_read() {
    let mask = png_of(ColorType::Grayscale, BitDepth::Sixteen, 2, 2, |x, _| {
        if x == 0 { vec![0xff, 0xff] } else { vec![0, 0] }
    });
    let (out, ..) = binary(&mask, BOUNDS).unwrap();
    assert_eq!(samples(&out), vec![255, 0, 255, 0]);
}

/// A mask cut off part-way through its rows is refused, not read as a shorter picture.
#[test]
fn a_mask_missing_rows_is_refused() {
    let mask = png_of(ColorType::Grayscale, BitDepth::Eight, 32, 32, |x, y| {
        vec![if (x + y) % 3 == 0 { 255 } else { 0 }]
    });
    // Drop the end of the image data and the IEND chunk.
    let cut = &mask[..mask.len() - 40];
    assert!(binary(cut, BOUNDS).is_err());
}

/// One translucent pixel on the FIRST row still applies to every later row.
#[test]
fn an_early_translucent_pixel_applies_to_later_rows() {
    let mask = png_of(ColorType::Rgba, BitDepth::Eight, 2, 3, |x, y| {
        if (x, y) == (0, 0) {
            vec![0, 0, 0, 0]
        } else {
            vec![255, 255, 255, 255]
        }
    });
    let (out, ..) = binary(&mask, BOUNDS).unwrap();
    assert_eq!(samples(&out), vec![255, 0, 0, 0, 0, 0]);
}
