//! The Gemini fill's total request size: references that are each within their own limit but
//! together pass the service's 20 MB request size are refused before anything is sent, against a
//! scripted server on this computer.
#![allow(dead_code)]
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_core::conformance::run_lifecycle_paced;
use craft_ai_core::payload::base64_encode;
use craft_ai_core::{
    CapabilityId, ErrorKind, InputRole, JobState, ModelId, Payload, Provider, Request, SecretString,
};
use craft_ai_gemini::{DEFAULT_MODEL, GeminiConfig, GeminiProvider};
use serde_json::json;
use support::{Reply, Server};

/// The side of each noise reference: about 5.9 MB as a PNG, under the 10 MB a reference may be.
const SIDE: u32 = 1400;

/// A short wait between polls.
fn pause() {
    std::thread::sleep(std::time::Duration::from_millis(2));
}

/// An RGB PNG of `side` x `side` pixels of noise from `seed`, which barely compresses, so its size
/// is close to three bytes a pixel.
fn noise_png(side: u32, seed: u32) -> Vec<u8> {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    let pixels: Vec<u8> = (0..side * side * 3)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state.to_le_bytes()[0]
        })
        .collect();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, side, side);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&pixels).unwrap();
    writer.finish().unwrap();
    out
}

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

/// A real 4 x 4 JPEG of one colour, the scripted answer.
fn jpeg(rgb: [u8; 3]) -> Vec<u8> {
    let picture = image::RgbImage::from_pixel(4, 4, image::Rgb(rgb));
    let mut out = std::io::Cursor::new(Vec::new());
    picture
        .write_to(&mut out, image::ImageFormat::Jpeg)
        .unwrap();
    out.into_inner()
}

/// A fill of a small source and an all-white mask, then `references` in that order.
fn request(references: &[Vec<u8>]) -> Request {
    let mut r = Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", png_rgb([10, 20, 30])).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", png_rgb([255, 255, 255])).unwrap(),
        )
        .with_input(InputRole::Prompt, Payload::text("a blue vase"));
    for reference in references {
        r = r.with_input(
            InputRole::Reference,
            Payload::inline("image/png", reference.clone()).unwrap(),
        );
    }
    r
}

/// Three references of about 5.9 MB each (17.6 MB, over 23 MB once encoded) are refused as an
/// invalid request with nothing sent; two of them (about 15.7 MB encoded) still go through, so the
/// refusal comes from the total, not from any one picture.
#[test]
fn references_that_pass_the_request_size_together_are_refused_before_sending() {
    let references: Vec<Vec<u8>> = (1..=3).map(|seed| noise_png(SIDE, seed)).collect();
    for r in &references {
        assert!(
            r.len() > 5_500_000 && r.len() < 10 * 1024 * 1024,
            "{}",
            r.len()
        );
    }
    let server = Server::start(|_| {
        Reply::Json(
            200,
            json!({"output_image": {"mime_type": "image/jpeg", "data": base64_encode(&jpeg([1, 2, 3]))}})
                .to_string(),
        )
    });
    let provider = GeminiProvider::new(GeminiConfig::new(
        &server.url,
        SecretString::new("AIza-fixture"),
    ))
    .unwrap();
    let model = ModelId::new(DEFAULT_MODEL).unwrap();

    let err = provider.submit(&model, &request(&references)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidRequest, "{}", err.message);
    assert!(err.message.contains("20 MB"), "{}", err.message);
    assert_eq!(
        server.count(),
        0,
        "nothing was sent for a request over the size"
    );

    let two = run_lifecycle_paced(&provider, &model, &request(&references[..2]), &pause);
    assert_eq!(
        two.unwrap().state,
        JobState::Succeeded,
        "two fit under the size"
    );
}
