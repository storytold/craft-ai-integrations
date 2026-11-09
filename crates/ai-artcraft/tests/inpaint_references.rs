//! Reference pictures through the Omni fill adapter, against a scripted server: upload order,
//! token order, the instruction that names them, and refusals made before any upload.
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_artcraft::{ArtCraftConfig, ArtCraftProvider};
use craft_ai_core::{
    CapabilityId, ErrorKind, InputRole, ModelId, Payload, Provider, Request, SecretString,
};
use support::{Reply, Server};

/// An 8 x 8 RGB PNG of one colour, so each picture's bytes are distinct.
fn png_rgb(rgb: [u8; 3]) -> Vec<u8> {
    let mut data = Vec::new();
    let mut e = png::Encoder::new(&mut data, 8, 8);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    let mut w = e.write_header().unwrap();
    w.write_image_data(&rgb.repeat(64)).unwrap();
    w.finish().unwrap();
    data
}

const SOURCE: [u8; 3] = [10, 20, 30];
const FIRST: [u8; 3] = [200, 0, 0];
const SECOND: [u8; 3] = [0, 200, 0];

/// A fill of a plain source and an all-white mask, then `references` in that order.
fn request(references: &[Payload]) -> Request {
    let mut r = Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", png_rgb(SOURCE)).unwrap(),
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

/// The real adapter, pointed at the scripted server.
fn provider(server: &Server) -> ArtCraftProvider {
    ArtCraftProvider::new(ArtCraftConfig::new(
        server.url.trim_end_matches("/v1"),
        SecretString::new("fixture-only-key"),
    ))
    .unwrap()
}

/// The token an upload is answered with, named after the picture it carries.
fn token_for(body: &[u8]) -> &'static str {
    let has = |p: Vec<u8>| body.windows(p.len()).any(|w| w == p.as_slice());
    if has(png_rgb(SOURCE)) {
        "m_source"
    } else if has(png_rgb(FIRST)) {
        "m_first"
    } else if has(png_rgb(SECOND)) {
        "m_second"
    } else {
        "m_mask"
    }
}

/// Two references are uploaded after the source and mask, their tokens follow in request order,
/// and the instruction names them as images 3 and 4.
#[test]
fn references_are_uploaded_and_listed_after_the_source_and_mask() {
    let server = Server::start(|r| match r.path.as_str() {
        "/v1/omni_api/upload/image" => Reply::Json(
            200,
            format!(
                r#"{{"success":true,"media_file_token":"{}"}}"#,
                token_for(&r.body)
            ),
        ),
        "/v1/omni_api/generate/image" => Reply::Json(
            200,
            r#"{"success":true,"inference_job_token":"jinf_refs"}"#.into(),
        ),
        _ => Reply::Json(500, "unexpected".into()),
    });
    let req = request(&[
        Payload::inline("image/png", png_rgb(FIRST)).unwrap(),
        Payload::inline("image/png", png_rgb(SECOND)).unwrap(),
    ]);
    provider(&server)
        .submit(&ModelId::new("nano_banana_pro").unwrap(), &req)
        .unwrap();
    let sent = server.recorded();
    let uploads: Vec<_> = sent
        .iter()
        .filter(|r| r.path.ends_with("upload/image"))
        .map(|r| token_for(&r.body))
        .collect();
    assert_eq!(uploads, ["m_source", "m_mask", "m_first", "m_second"]);
    let body = sent.last().unwrap().body_json();
    assert_eq!(body["model"], "nano_banana_pro", "the chosen model is kept");
    assert_eq!(
        body["image_media_tokens"],
        serde_json::json!(["m_source", "m_mask", "m_first", "m_second"])
    );
    let prompt = body["prompt"].as_str().unwrap();
    assert!(
        prompt.contains("Images 3 to 4 are reference pictures"),
        "{prompt}"
    );
}

/// A JPEG (Omni uploads PNG only), a broken PNG and a link are refused before any upload. The count
/// is Omni's to judge, so it is not checked here.
#[test]
fn bad_references_are_refused_before_any_upload() {
    let server = Server::start(|_| Reply::Json(500, "unexpected".into()));
    let p = provider(&server);
    let model = ModelId::new("nano_banana_2").unwrap();
    let mut cut = png_rgb(SECOND);
    cut.truncate(cut.len() - 20);
    // Refused for its format, before its body is read: the error names PNG, not a broken file.
    let jpeg_header = [0xff, 0xd8, 0xff, 0xe0, 0, 16, b'J', b'F', b'I', b'F', 0, 1];
    let jpeg = Payload::inline("image/jpeg", jpeg_header.to_vec()).unwrap();
    let err = p.submit(&model, &request(&[jpeg])).unwrap_err();
    assert!(err.message.contains("must be a PNG"), "{}", err.message);
    let cases = [
        (
            Payload::inline("image/png", cut).unwrap(),
            ErrorKind::InvalidRequest,
        ),
        (
            Payload::handle("https://example.com/ref.png", "image/png", None).unwrap(),
            ErrorKind::Unsupported,
        ),
    ];
    for (reference, kind) in cases {
        let err = p.submit(&model, &request(&[reference])).unwrap_err();
        assert_eq!(err.kind, kind, "{}", err.message);
    }
    assert_eq!(server.count(), 0, "nothing was uploaded");
}

/// Each Banana model declares optional references for a fill with no count of its own, and none
/// for generation.
#[test]
fn banana_fill_declares_optional_references() {
    let fill = CapabilityId::parse("image.inpaint").unwrap();
    let generate = CapabilityId::parse("image.generate").unwrap();
    for name in ["nano_banana", "nano_banana_2", "nano_banana_pro"] {
        let model = ArtCraftProvider::documented_models()
            .into_iter()
            .find(|m| m.id.as_str() == name)
            .unwrap();
        let spec = model.spec_for(&fill).unwrap();
        assert_eq!(
            spec.accepts.get(&InputRole::Reference),
            Some(&u32::MAX),
            "{name}"
        );
        assert!(!spec.requires.contains(&InputRole::Reference), "{name}");
        let generate = model.spec_for(&generate).unwrap();
        assert!(
            !generate.accepts.contains_key(&InputRole::Reference),
            "{name}"
        );
    }
}
