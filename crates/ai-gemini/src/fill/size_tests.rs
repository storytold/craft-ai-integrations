//! The request-size limit at its edge: a body of exactly [`MAX_REQUEST_BYTES`] is allowed and one
//! byte more is refused, measured on the bytes that would be sent.
#![allow(clippy::unwrap_used)]
use super::*;
use craft_ai_core::CapabilityId;

/// A one-colour 8 x 8 RGB PNG.
fn png_rgb(rgb: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, 8, 8);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&rgb.repeat(64)).unwrap();
    writer.finish().unwrap();
    out
}

/// A fill of a small source and an all-white mask with `prompt`.
fn request(prompt: &str) -> Request {
    Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", png_rgb([10, 20, 30])).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", png_rgb([255, 255, 255])).unwrap(),
        )
        .with_input(InputRole::Prompt, Payload::text(prompt))
}

/// The body's serialised length for `prompt`, measured without the limit.
fn sent_len(prompt: &str) -> usize {
    serde_json::to_vec(&body(MODELS[0].0, &request(prompt)).unwrap())
        .unwrap()
        .len()
}

/// Control characters are escaped to six bytes each when sent, so a prompt of about 3.3 million of
/// them (under the 4 MiB text limit, and far under 20 MB as raw bytes) brings the body to the
/// limit. Padding with plain letters then lands it exactly on [`MAX_REQUEST_BYTES`], which is
/// allowed; one letter more is refused before anything is sent.
#[test]
fn the_limit_is_measured_on_the_bytes_sent_and_holds_to_the_byte() {
    let controls = "\u{1}".repeat(3_300_000);
    let short = sent_len(&controls);
    assert!(controls.len() < 4 * 1024 * 1024 && short < MAX_REQUEST_BYTES);
    assert!(
        short > 6 * controls.len(),
        "each control character counts as its six escaped bytes"
    );

    let at_limit = format!("{controls}{}", "y".repeat(MAX_REQUEST_BYTES - short));
    assert_eq!(
        sent_len(&at_limit),
        MAX_REQUEST_BYTES,
        "the prompt appears once"
    );
    let sent = encoded_body(MODELS[0].0, &request(&at_limit)).unwrap();
    assert_eq!(sent.len(), MAX_REQUEST_BYTES, "the limit itself is allowed");

    let over = format!("{at_limit}y");
    let err = encoded_body(MODELS[0].0, &request(&over)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidRequest, "{}", err.message);
    assert!(err.message.contains("20 MB"), "{}", err.message);
}
