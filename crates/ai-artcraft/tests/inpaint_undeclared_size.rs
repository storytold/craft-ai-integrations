//! An Omni result sent without a Content-Length is still held to the size limit, before it is
//! decoded, and fails with the same error as one whose declared size is too large.
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_artcraft::{ArtCraftConfig, ArtCraftProvider};
use craft_ai_core::{
    CapabilityId, ErrorKind, InputRole, JobState, Limits, ModelId, Payload, Provider, Request,
    SecretString,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use support::{Reply, Server};

/// An all-white 8-bit RGBA PNG.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut data = Vec::new();
    {
        let mut e = png::Encoder::new(&mut data, width, height);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header()
            .unwrap()
            .write_image_data(&vec![255; (width * height * 4) as usize])
            .unwrap();
    }
    data
}

/// A fill request with an 8 × 8 source and mask.
fn request() -> Request {
    Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", png(8, 8)).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", png(8, 8)).unwrap(),
        )
        .with_input(InputRole::Prompt, Payload::text("a small red balloon"))
}

/// The result body: a whole, readable PNG padded to one byte over the inline limit. The PNG
/// decoder stops at the end of the picture, so only a size check can refuse it as too large.
fn oversized_result() -> Vec<u8> {
    let limit = usize::try_from(Limits::default().max_inline_bytes).unwrap();
    let mut body = png(8, 8);
    body.resize(limit + 1, 0);
    body
}

/// A delivery with no Content-Length that is one byte over the limit fails as too large.
#[test]
fn an_undeclared_oversized_result_is_refused_as_too_large() {
    let n = Arc::new(AtomicUsize::new(0));
    let server = Server::start(move |r| match (r.method.as_str(), r.path.as_str()) {
        ("POST", "/v1/omni_api/upload/image") => Reply::Json(
            200,
            format!(
                r#"{{"success":true,"media_file_token":"m_{}"}}"#,
                n.fetch_add(1, Ordering::SeqCst)
            ),
        ),
        ("POST", "/v1/omni_api/generate/image") => Reply::Json(
            200,
            r#"{"success":true,"inference_job_token":"jinf_big"}"#.into(),
        ),
        ("GET", "/v1/omni_api/job_status/job/jinf_big") => {
            let uri = format!("http://{}/result.png", r.header("host").unwrap());
            Reply::Json(200, serde_json::json!({"success":true,"state":{"job_token":"jinf_big",
                "status":{"status":"complete_success"},"maybe_result":{"media_links":{"cdn_url":uri}}}}).to_string())
        }
        ("GET", "/result.png") => {
            let mut raw = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
            raw.extend_from_slice(&oversized_result());
            Reply::Raw(raw)
        }
        _ => panic!("unexpected request"),
    });
    let p = ArtCraftProvider::new(ArtCraftConfig::new(
        server.url.trim_end_matches("/v1"),
        SecretString::new("fixture-only-key"),
    ))
    .unwrap();
    let id = p
        .submit(&ModelId::new("nano_banana_2").unwrap(), &request())
        .unwrap();
    let JobState::Failed { error } = p.poll(&id).unwrap().state else {
        panic!("an oversized result must fail the job");
    };
    assert_eq!(error.kind, ErrorKind::Provider);
    assert_eq!(error.message, "ArtCraft result image is too large");
}
