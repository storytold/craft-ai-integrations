//! The size check made before a body is built: it refuses from the input sizes alone, without
//! decoding a picture, it holds to the byte, and it never counts more than would be sent.
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

/// A fill of `source` with a small mask and one reference picture per entry of `references`.
fn request(source: Vec<u8>, references: &[Vec<u8>]) -> Request {
    let mut request = Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", source).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", png_rgb([255, 255, 255])).unwrap(),
        )
        .with_input(InputRole::Prompt, Payload::text("a red door"));
    for r in references {
        request = request.with_input(
            InputRole::Reference,
            Payload::inline("image/png", r.clone()).unwrap(),
        );
    }
    request
}

/// Bytes that are not a picture at all, so building the body would fail on the format. A refusal
/// for size proves the check ran before any picture was read.
fn not_a_picture(len: usize) -> Vec<u8> {
    vec![0; len]
}

/// A source of 6,000,000 bytes and three references of 3,000,000 each come to exactly 20,000,000
/// bytes of base64, which is allowed. One byte more on the source adds a base64 group and is
/// refused, and `encoded_body` gives the same size refusal instead of a format error, so it never
/// started building the body.
#[test]
fn the_check_refuses_from_sizes_alone_and_holds_to_the_byte() {
    let refs = vec![not_a_picture(3_000_000); 3];
    let at_limit = request(not_a_picture(6_000_000), &refs);
    assert_eq!(lower_bound(&at_limit), MAX_REQUEST_BYTES);
    assert!(preflight(&at_limit).is_ok(), "the limit itself is allowed");

    let over = request(not_a_picture(6_000_001), &refs);
    assert_eq!(lower_bound(&over), MAX_REQUEST_BYTES + 4);
    let err = preflight(&over).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidRequest, "{}", err.message);
    assert!(err.message.contains("20 MB"), "{}", err.message);

    let err = encoded_body(MODELS[0].0, &over).unwrap_err();
    assert!(err.message.contains("20 MB"), "{}", err.message);
}

/// For a real fill the bound is the base64 length of the source and references and no more: the
/// body sent is longer, because the mask, the instruction and the fields around them come on top.
#[test]
fn the_bound_counts_source_and_references_and_stays_under_the_bytes_sent() {
    let source = png_rgb([10, 20, 30]);
    let refs = vec![png_rgb([200, 0, 0]), png_rgb([0, 200, 0])];
    let fill = request(source.clone(), &refs);
    let expected: usize = std::iter::once(&source)
        .chain(&refs)
        .map(|b| base64_encode(b).len())
        .sum();
    assert_eq!(lower_bound(&fill), expected);
    let sent = encoded_body(MODELS[0].0, &fill).unwrap();
    assert!(lower_bound(&fill) < sent.len(), "{} bytes sent", sent.len());
}
