//! FLUX.1 Fill takes no reference pictures: BFL's fill endpoint has an image, a mask and a prompt
//! and nothing else. A reference is refused before anything is sent, never dropped quietly.
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_bfl::{BflConfig, BflProvider};
use craft_ai_core::{
    CapabilityId, ErrorKind, InputRole, ModelId, Payload, Provider, Request, SecretString,
};
use support::{Reply, Server};

/// An 8 x 8 grayscale PNG of one shade.
fn gray_png(shade: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, 8, 8);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&[shade; 64]).unwrap();
    writer.finish().unwrap();
    out
}

/// A valid fill with one reference picture added.
fn fill_with_reference() -> Request {
    Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", gray_png(128)).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", gray_png(255)).unwrap(),
        )
        .with_input(InputRole::Prompt, Payload::text("a red door"))
        .with_input(
            InputRole::Reference,
            Payload::inline("image/png", gray_png(40)).unwrap(),
        )
}

/// The fill model declares no reference input, so the client's route check refuses it too.
#[test]
fn flux_fill_declares_no_reference_input() {
    let cap = CapabilityId::parse("image.inpaint").unwrap();
    let fill = BflProvider::catalog()
        .into_iter()
        .find(|m| m.id.as_str() == "flux-pro-1.0-fill")
        .unwrap();
    let spec = fill.spec_for(&cap).unwrap();
    assert!(!spec.accepts.contains_key(&InputRole::Reference));
}

/// A fill carrying a reference is refused by `submit` and nothing reaches the server.
#[test]
fn a_reference_is_refused_before_sending() {
    let server = Server::start(|_| Reply::Json(500, "unexpected".into()));
    let p = BflProvider::new(BflConfig::new(&server.url, SecretString::new("k"))).unwrap();
    let err = p
        .submit(
            &ModelId::new("flux-pro-1.0-fill").unwrap(),
            &fill_with_reference(),
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unsupported, "{}", err.message);
    assert_eq!(server.count(), 0);
}
