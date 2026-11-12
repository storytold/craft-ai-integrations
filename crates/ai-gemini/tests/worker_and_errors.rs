//! Gemini fill cases from review: a cancel that must outlast the worker's late answer, a key cut
//! in half by the error-length limit, and a Lite model asked for a size it cannot make.
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_core::conformance::{run_lifecycle_paced, run_rejects};
use craft_ai_core::payload::base64_encode;
use craft_ai_core::{
    CapabilityId, ErrorKind, InputRole, JobState, ModelId, Payload, Provider, Request, SecretString,
};
use craft_ai_gemini::{DEFAULT_MODEL, GeminiConfig, GeminiProvider};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use support::{Reply, Server};

const KEY: &str = "AIza-gemini-needle";

/// A short wait between polls.
fn pause() {
    std::thread::sleep(Duration::from_millis(2));
}

/// An 8 × 8 PNG of `color` whose pixels all come from `pixel`.
fn png(color: png::ColorType, pixel: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, 8, 8);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&pixel.repeat(64)).unwrap();
    writer.finish().unwrap();
    out
}

/// A fill of an 8 × 8 picture with an all-white mask.
fn fill_request() -> Request {
    Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", png(png::ColorType::Rgb, &[10, 20, 30])).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", png(png::ColorType::Grayscale, &[255])).unwrap(),
        )
        .with_input(InputRole::Prompt, Payload::text("a red door"))
}

/// The real adapter with the fixture key, pointed at `url`.
fn provider(url: &str) -> GeminiProvider {
    GeminiProvider::new(GeminiConfig::new(url, SecretString::new(KEY))).unwrap()
}

/// Cancelled stays cancelled after the worker's request comes back with a real picture: the
/// server holds its answer until after the cancel, then the job is watched well past the answer.
#[test]
fn a_cancelled_fill_stays_cancelled_after_the_worker_finishes() {
    let release = Arc::new(AtomicBool::new(false));
    let answered = Arc::new(AtomicBool::new(false));
    let (hold, done) = (release.clone(), answered.clone());
    let server = Server::start(move |_| {
        while !hold.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        done.store(true, Ordering::SeqCst);
        Reply::Json(
            200,
            json!({"steps": [{"type": "model_output", "content": [
                {"type": "image", "mime_type": "image/jpeg",
                 "data": base64_encode(b"\xff\xd8\xff\xe0 a jpeg")}
            ]}]})
            .to_string(),
        )
    });
    let p = provider(&server.url);
    let job = p
        .submit(&ModelId::new(DEFAULT_MODEL).unwrap(), &fill_request())
        .unwrap();
    p.cancel(&job).unwrap();
    assert_eq!(p.poll(&job).unwrap().state, JobState::Cancelled);
    release.store(true, Ordering::SeqCst);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !answered.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "the server never answered");
        pause();
    }
    // The worker reads the answer and records it within milliseconds; watch for well over that.
    let watch = Instant::now() + Duration::from_millis(500);
    while Instant::now() < watch {
        assert_eq!(p.poll(&job).unwrap().state, JobState::Cancelled);
        pause();
    }
    assert_eq!(server.count(), 1);
}

/// The key is scrubbed from Google's whole message before it is cut to length, so a key that
/// straddles the cut leaves no fragment in the error a person sees.
#[test]
fn a_key_across_the_length_limit_leaves_no_fragment() {
    let message = format!("{}{KEY} was rejected", "x".repeat(290));
    let server = Server::start(move |_| {
        Reply::Json(500, json!({"error": {"message": message}}).to_string())
    });
    let p = provider(&server.url);
    let done = run_lifecycle_paced(
        &p,
        &ModelId::new(DEFAULT_MODEL).unwrap(),
        &fill_request(),
        &pause,
    )
    .unwrap();
    let JobState::Failed { error } = done.state else {
        panic!("the job should fail: {:?}", done.state);
    };
    assert!(!error.message.contains(&KEY[..5]), "{}", error.message);
}

/// A Lite model refuses 2K and 4K before anything is sent, and still takes 1K.
#[test]
fn a_lite_model_refuses_a_size_it_cannot_make() {
    let server = Server::start(|_| Reply::Json(500, "{}".into()));
    let p = provider(&server.url);
    let lite = ModelId::new("gemini-3.1-flash-lite-image").unwrap();
    for size in ["2K", "4K"] {
        let request = fill_request().with_param("image_size", json!(size));
        run_rejects(&p, &lite, &request, ErrorKind::Unsupported).unwrap();
    }
    assert_eq!(server.count(), 0);
    assert!(
        p.submit(&lite, &fill_request().with_param("image_size", json!("1K")))
            .is_ok()
    );
}

/// `n` tiny objects, comma-separated: about 7 bytes each on the wire, about 800 once parsed
/// as a plain `serde_json::Value`.
fn tiny_objects(n: usize) -> String {
    vec![r#"{"":0}"#; n].join(",")
}

/// A reply's cost once parsed is bounded, not only its bytes: about 1 MiB of tiny objects (some
/// 120 MiB as a plain `Value`) is refused, from the model list and from a fill.
#[test]
fn a_reply_of_tiny_objects_is_refused() {
    let listing = format!(r#"{{"models":[],"extra":[{}]}}"#, tiny_objects(150_000));
    let server = Server::start(move |_| Reply::Json(200, listing.clone()));
    let e = provider(&server.url).models().unwrap_err();
    assert_eq!(e.kind, ErrorKind::Provider);
    assert!(e.message.contains("too large"), "{}", e.message);

    let answer = format!(
        r#"{{"steps":[{{"type":"model_output","content":[]}}],"extra":[{}]}}"#,
        tiny_objects(150_000)
    );
    let server = Server::start(move |_| Reply::Json(200, answer.clone()));
    let p = provider(&server.url);
    let done = run_lifecycle_paced(
        &p,
        &ModelId::new(DEFAULT_MODEL).unwrap(),
        &fill_request(),
        &pause,
    )
    .unwrap();
    match done.state {
        JobState::Failed { error } => {
            assert!(error.message.contains("too large"), "{}", error.message)
        }
        other => panic!("expected a failure, got {other:?}"),
    }
}
