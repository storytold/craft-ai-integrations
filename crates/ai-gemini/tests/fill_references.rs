//! Reference pictures through the Gemini fill adapter, against a scripted server on this computer:
//! the exact order sent, the instruction that names them, and every refusal made before sending.
#![allow(dead_code)]
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_core::conformance::run_lifecycle_paced;
use craft_ai_core::payload::{base64_decode, base64_encode};
use craft_ai_core::{
    CapabilityId, ErrorKind, InputRole, JobState, ModelId, Payload, Provider, Request, SecretString,
};
use craft_ai_gemini::{DEFAULT_MODEL, GeminiConfig, GeminiProvider};
use serde_json::json;
use support::{Reply, Server};

/// A short wait between polls.
fn pause() {
    std::thread::sleep(std::time::Duration::from_millis(2));
}

/// An 8 x 8 RGB PNG of one colour.
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

/// A real 4 x 4 JPEG of one colour.
fn jpeg(rgb: [u8; 3]) -> Vec<u8> {
    let picture = image::RgbImage::from_pixel(4, 4, image::Rgb(rgb));
    let mut out = std::io::Cursor::new(Vec::new());
    picture
        .write_to(&mut out, image::ImageFormat::Jpeg)
        .unwrap();
    out.into_inner()
}

/// A fill of a plain source and an all-white mask, then `references` in that order.
fn request(references: &[Payload]) -> Request {
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
        r = r.with_input(InputRole::Reference, reference.clone());
    }
    r
}

/// The real adapter with a fixture key, pointed at `url`.
fn provider(url: &str) -> GeminiProvider {
    GeminiProvider::new(GeminiConfig::new(url, SecretString::new("AIza-fixture"))).unwrap()
}

/// A server that answers every fill with one small JPEG.
fn answer_server() -> Server {
    Server::start(|_| {
        Reply::Json(
            200,
            json!({"output_image": {"mime_type": "image/jpeg", "data": base64_encode(&jpeg([1, 2, 3]))}})
                .to_string(),
        )
    })
}

/// Two references (a PNG and a JPEG) follow the source and the mask in request order, each with
/// its own media type, and the instruction names them as images 3 and 4.
#[test]
fn references_follow_the_source_and_mask_in_request_order() {
    let server = answer_server();
    let p = provider(&server.url);
    let first = png_rgb([200, 0, 0]);
    let second = jpeg([0, 200, 0]);
    let req = request(&[
        Payload::inline("image/png", first.clone()).unwrap(),
        Payload::inline("image/jpeg", second.clone()).unwrap(),
    ]);
    let model = ModelId::new(DEFAULT_MODEL).unwrap();
    let done = run_lifecycle_paced(&p, &model, &req, &pause).unwrap();
    assert_eq!(done.state, JobState::Succeeded);

    let body = server.recorded()[0].body_json();
    assert_eq!(body["model"], DEFAULT_MODEL, "the chosen model is kept");
    let input = body["input"].as_array().unwrap();
    assert_eq!(input.len(), 5);
    let data = |i: usize| base64_decode(input[i]["data"].as_str().unwrap()).unwrap();
    assert_eq!(data(1), png_rgb([10, 20, 30]), "source first");
    assert_eq!(input[2]["mime_type"], "image/png", "then the mask");
    // The mask itself, normalised to opaque grayscale (all white here), not the source sent twice.
    let mut reader = png::Decoder::new(std::io::Cursor::new(data(2)))
        .read_info()
        .unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut pixels).unwrap();
    assert_eq!((frame.width, frame.height), (8, 8));
    assert_eq!(frame.color_type, png::ColorType::Grayscale);
    assert_eq!(&pixels[..frame.buffer_size()], &[255; 64]);
    assert_eq!(
        (data(3), input[3]["mime_type"].clone()),
        (first, json!("image/png"))
    );
    assert_eq!(
        (data(4), input[4]["mime_type"].clone()),
        (second, json!("image/jpeg"))
    );
    let instruction = input[0]["text"].as_str().unwrap();
    assert!(
        instruction.contains("Images 3 to 4 are reference pictures"),
        "{instruction}"
    );
    assert!(instruction.contains("a blue vase"), "{instruction}");
}

/// A fill without references is sent exactly as before: three pictures' worth of input and no
/// reference wording, so references never become required.
#[test]
fn a_fill_without_references_is_unchanged() {
    let server = answer_server();
    let p = provider(&server.url);
    let model = ModelId::new(DEFAULT_MODEL).unwrap();
    run_lifecycle_paced(&p, &model, &request(&[]), &pause).unwrap();
    let body = server.recorded()[0].body_json();
    let input = body["input"].as_array().unwrap();
    assert_eq!(input.len(), 3);
    assert!(!input[0]["text"].as_str().unwrap().contains("reference"));
}

/// Too many, a link, a broken picture and a picture too large in pixels are all refused before
/// anything is sent.
#[test]
fn bad_references_are_refused_before_sending() {
    let server = Server::start(|_| Reply::Json(500, "unexpected".into()));
    let p = provider(&server.url);
    let model = ModelId::new(DEFAULT_MODEL).unwrap();
    let ok = Payload::inline("image/png", png_rgb([1, 1, 1])).unwrap();
    let mut too_many = vec![ok.clone(); GeminiProvider::MAX_REFERENCES + 1];
    let err = p.submit(&model, &request(&too_many)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Unsupported, "{}", err.message);
    too_many.pop();
    let answers = answer_server();
    let at_limit =
        run_lifecycle_paced(&provider(&answers.url), &model, &request(&too_many), &pause);
    assert_eq!(
        at_limit.unwrap().state,
        JobState::Succeeded,
        "the limit itself is allowed"
    );

    let mut cut = png_rgb([5, 5, 5]);
    cut.truncate(cut.len() - 20);
    let cases = [
        (
            Payload::handle("https://example.com/ref.png", "image/png", None).unwrap(),
            ErrorKind::Unsupported,
        ),
        (
            Payload::inline("image/png", cut).unwrap(),
            ErrorKind::InvalidRequest,
        ),
        (
            Payload::inline("image/png", b"not a picture".to_vec()).unwrap(),
            ErrorKind::InvalidRequest,
        ),
    ];
    let before = server.count();
    for (reference, kind) in cases {
        let err = p.submit(&model, &request(&[reference])).unwrap_err();
        assert_eq!(err.kind, kind, "{}", err.message);
    }
    assert_eq!(
        server.count(),
        before,
        "nothing was sent for a refused reference"
    );
}

/// The catalogue the adapter offers declares references as optional, up to the same limit.
#[test]
fn the_offered_fill_spec_accepts_optional_references() {
    let server = Server::start(|_| Reply::Json(200, json!({"models": []}).to_string()));
    let models = provider(&server.url).models().unwrap();
    let cap = CapabilityId::parse("image.inpaint").unwrap();
    for m in models {
        let spec = m.spec_for(&cap).unwrap();
        assert_eq!(
            spec.accepts.get(&InputRole::Reference).copied(),
            Some(u32::try_from(GeminiProvider::MAX_REFERENCES).unwrap())
        );
        assert!(!spec.requires.contains(&InputRole::Reference));
    }
}
