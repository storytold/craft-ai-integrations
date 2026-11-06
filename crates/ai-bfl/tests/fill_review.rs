//! FLUX Fill cases from review, against a scripted server: a busy result download is tried again,
//! a found task ends the not-found streak even when its download fails, and a JPEG source is held
//! to the same size rules as a PNG.
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_bfl::{BflConfig, BflProvider};
use craft_ai_core::{
    CapabilityId, ErrorKind, InputRole, JobState, ModelId, Payload, Provider, Request, SecretString,
};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use support::{Reply, Server, http_bytes};

/// An 8-bit grayscale PNG of one shade.
fn gray_png(width: u32, height: u32, shade: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer
        .write_image_data(&vec![shade; (width * height) as usize])
        .unwrap();
    writer.finish().unwrap();
    out
}

/// The start of a JPEG: SOI and a baseline frame header stating `width` × `height`.
fn jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut d = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08];
    d.extend_from_slice(&height.to_be_bytes());
    d.extend_from_slice(&width.to_be_bytes());
    d.extend_from_slice(&[0x03; 12]);
    d
}

/// A fill of `source` (of `media` type) with an 8 × 8 all-white mask.
fn fill_request(media: &str, source: Vec<u8>) -> Request {
    Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(InputRole::Source, Payload::inline(media, source).unwrap())
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", gray_png(8, 8, 255)).unwrap(),
        )
        .with_input(InputRole::Prompt, Payload::text("a red door"))
}

/// The real adapter with a fixture key and no not-found grace, pointed at `url`.
fn provider(url: &str) -> BflProvider {
    let mut config = BflConfig::new(url, SecretString::new("fixture-only-key"));
    config.not_found_grace = Duration::ZERO;
    BflProvider::new(config).unwrap()
}

/// The FLUX.1 Fill [pro] model id.
fn model() -> ModelId {
    ModelId::new("flux-pro-1.0-fill").unwrap()
}

/// A server whose poll `n` (from 0) answers `status(n)`, a Ready answer pointing at its own
/// `/delivery/sample.png`, which answers `download(m)` on its `m`th fetch.
fn server(
    status: impl Fn(u32) -> &'static str + Send + Sync + 'static,
    download: impl Fn(u32) -> u16 + Send + Sync + 'static,
) -> Server {
    let (polls, fetches) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
    Server::start(move |r| match (r.method.as_str(), r.path.as_str()) {
        ("POST", "/v1/flux-pro-1.0-fill") => {
            Reply::Json(200, r#"{"id":"fill-1","polling_url":"ignored"}"#.into())
        }
        ("GET", "/v1/get_result?id=fill-1") => {
            let state = status(polls.fetch_add(1, Ordering::SeqCst));
            let url = format!("http://{}/delivery/sample.png", r.header("host").unwrap());
            Reply::Json(
                200,
                json!({"id":"fill-1","status":state,"result":{"sample":url}}).to_string(),
            )
        }
        ("GET", "/delivery/sample.png") => match download(fetches.fetch_add(1, Ordering::SeqCst)) {
            200 => http_bytes(200, &[("Content-Type", "image/png")], &gray_png(8, 8, 200)),
            code => Reply::Json(code, "{}".into()),
        },
        _ => Reply::Json(404, "{}".into()),
    })
}

/// 503 and 429 on the result download are passing failures: the poll errors with a retryable kind
/// and the next poll fetches the picture.
#[test]
fn a_busy_result_download_is_tried_again() {
    let s = server(|_| "Ready", |n| [503, 429, 200][n as usize]);
    let p = provider(&s.url);
    let id = p
        .submit(&model(), &fill_request("image/png", gray_png(8, 8, 128)))
        .unwrap();
    assert_eq!(p.poll(&id).unwrap_err().kind, ErrorKind::Network);
    assert_eq!(p.poll(&id).unwrap_err().kind, ErrorKind::RateLimited);
    assert_eq!(p.poll(&id).unwrap().state, JobState::Succeeded);
}

/// An expired link (403) will not come back, so the job ends as failed.
#[test]
fn an_expired_result_link_fails_the_job() {
    let s = server(|_| "Ready", |_| 403);
    let p = provider(&s.url);
    let id = p
        .submit(&model(), &fill_request("image/png", gray_png(8, 8, 128)))
        .unwrap();
    assert!(matches!(
        p.poll(&id).unwrap().state,
        JobState::Failed { .. }
    ));
}

/// Nine not-found answers, a Ready whose download is busy, then nine more: the Ready ended the
/// first streak, so the job is still waiting rather than failed after the tenth not-found.
#[test]
fn a_ready_answer_ends_the_not_found_streak_even_if_its_download_fails() {
    let s = server(|n| if n == 9 { "Ready" } else { "Task not found" }, |_| 503);
    let p = provider(&s.url);
    let id = p
        .submit(&model(), &fill_request("image/png", gray_png(8, 8, 128)))
        .unwrap();
    for _ in 0..9 {
        assert_eq!(p.poll(&id).unwrap().state, JobState::Queued);
    }
    assert_eq!(p.poll(&id).unwrap_err().kind, ErrorKind::Network);
    for _ in 0..9 {
        assert_eq!(p.poll(&id).unwrap().state, JobState::Queued);
    }
    assert!(matches!(
        p.poll(&id).unwrap().state,
        JobState::Failed { .. }
    ));
}

/// A JPEG source is measured too: one the mask's size is accepted, a different size or one too
/// large is refused before anything is sent.
#[test]
fn a_jpeg_source_meets_the_same_size_rules_as_a_png() {
    let s = Server::start(|_| Reply::Json(500, "{}".into()));
    let p = provider(&s.url);
    for source in [jpeg(16, 16), jpeg(9000, 8)] {
        let err = p
            .submit(&model(), &fill_request("image/jpeg", source))
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidRequest);
    }
    assert_eq!(s.count(), 0);
    let err = p
        .submit(&model(), &fill_request("image/jpeg", jpeg(8, 8)))
        .unwrap_err();
    assert_ne!(err.kind, ErrorKind::InvalidRequest, "{err:?}");
    assert_eq!(s.count(), 1, "a matching JPEG is sent");
}
