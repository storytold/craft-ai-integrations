//! FLUX.1 Fill [pro] against a scripted server on this computer: no real network, no real key.
#![allow(dead_code)]
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_bfl::{BflConfig, BflProvider};
use craft_ai_core::conformance::{
    run_cancel_paced, run_lifecycle_paced, run_rejects, run_unknown_job,
};
use craft_ai_core::payload::base64_decode;
use craft_ai_core::{
    CapabilityId, ErrorKind, Event, InputRole, JobState, ModelId, Payload, Provider, Request,
    SecretString,
};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use support::{Recorded, Reply, Server, http_bytes};

const KEY: &str = "fill-needle-key";

fn pause() {
    std::thread::sleep(std::time::Duration::from_millis(2));
}

fn gray_png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> u8) -> Vec<u8> {
    let data: Vec<u8> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| pixel(x, y))
        .collect();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&data).unwrap();
    writer.finish().unwrap();
    out
}

fn model() -> ModelId {
    ModelId::new("flux-pro-1.0-fill").unwrap()
}

fn fill_request() -> Request {
    Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", gray_png(8, 8, |_, _| 128)).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline(
                "image/png",
                gray_png(8, 8, |x, _| if x < 4 { 255 } else { 0 }),
            )
            .unwrap(),
        )
        .with_input(InputRole::Prompt, Payload::text("a red door"))
}

fn provider(url: &str) -> BflProvider {
    BflProvider::new(BflConfig::new(url, SecretString::new(KEY))).unwrap()
}

fn result_png() -> Vec<u8> {
    gray_png(8, 8, |_, _| 200)
}

/// The address of `path` on the server that received `r`.
fn on_server(r: &Recorded, path: &str) -> String {
    format!("http://{}{path}", r.header("host").unwrap())
}

/// A fill that is pending once, generating once, then ready with a picture on the same server.
fn fill_server() -> Server {
    let polls = Arc::new(AtomicU32::new(0));
    Server::start(move |r| match (r.method.as_str(), r.path.as_str()) {
        ("POST", "/v1/flux-pro-1.0-fill") => {
            Reply::Json(200, r#"{"id":"fill-1","polling_url":"ignored"}"#.into())
        }
        ("GET", "/v1/get_result?id=fill-1") => {
            let status = match polls.fetch_add(1, Ordering::SeqCst) {
                0 => r#""Pending""#.to_owned(),
                1 => r#""Generating""#.to_owned(),
                _ => {
                    let url = on_server(r, "/delivery/sample.png?sig=abc");
                    return Reply::Json(
                        200,
                        json!({"id":"fill-1","status":"Ready","result":{"sample":url}}).to_string(),
                    );
                }
            };
            Reply::Json(200, format!(r#"{{"id":"fill-1","status":{status}}}"#))
        }
        ("GET", "/delivery/sample.png?sig=abc") => {
            http_bytes(200, &[("Content-Type", "image/png")], &result_png())
        }
        _ => Reply::Json(404, "{}".into()),
    })
}

#[test]
fn the_fill_model_is_listed_for_inpainting_with_source_mask_and_prompt() {
    let server = Server::start(|_| Reply::Json(200, r#"{"credits":3}"#.into()));
    let models = provider(&server.url).models().unwrap();
    let fill = models.iter().find(|m| m.id == model()).unwrap();
    let spec = fill
        .spec_for(&CapabilityId::parse("image.inpaint").unwrap())
        .unwrap();
    for role in [InputRole::Source, InputRole::Mask, InputRole::Prompt] {
        assert!(spec.requires.contains(&role), "{role:?}");
    }
}

#[test]
fn a_fill_is_submitted_polled_and_downloaded_inline_without_the_key() {
    let server = fill_server();
    let p = provider(&server.url);
    let done = run_lifecycle_paced(&p, &model(), &fill_request(), &pause).unwrap();
    assert_eq!(done.state, JobState::Succeeded);
    let outputs: Vec<_> = done
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Output(o) => Some(o),
            _ => None,
        })
        .collect();
    assert_eq!(outputs.len(), 1);
    assert!(matches!(&outputs[0].payload,
        Payload::Inline { media_type, data } if media_type.as_str() == "image/png" && *data == result_png()));

    let recorded = server.recorded();
    let submit = recorded.iter().find(|r| r.method == "POST").unwrap();
    assert_eq!(submit.header("x-key"), Some(KEY));
    let body = submit.body_json();
    assert_eq!(body["prompt"], "a red door");
    assert_eq!(body["output_format"], "png");
    let mask = base64_decode(body["mask"].as_str().unwrap()).unwrap();
    assert!(mask.starts_with(&[0x89, b'P', b'N', b'G']));
    let download = recorded
        .iter()
        .find(|r| r.path.starts_with("/delivery/"))
        .unwrap();
    assert_eq!(
        download.header("x-key"),
        None,
        "the key never goes to a result link"
    );
}

#[test]
fn conformance_unknown_jobs_rejects_and_cancel() {
    let server = fill_server();
    let p = provider(&server.url);
    run_unknown_job(&p).unwrap();
    let no_mask = Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", gray_png(8, 8, |_, _| 1)).unwrap(),
        )
        .with_input(InputRole::Prompt, Payload::text("door"));
    run_rejects(&p, &model(), &no_mask, ErrorKind::InvalidRequest).unwrap();
    let generate = Request::new(CapabilityId::parse("image.generate").unwrap())
        .with_input(InputRole::Prompt, Payload::text("door"));
    run_rejects(&p, &model(), &generate, ErrorKind::Unsupported).unwrap();
    assert_eq!(server.count(), 0, "refused before anything was sent");

    // BFL has no cancel route: an unfinished fill says so, a finished one cancels quietly.
    let id = p.submit(&model(), &fill_request()).unwrap();
    assert_eq!(p.cancel(&id).unwrap_err().kind, ErrorKind::Unsupported);
    let ready = Server::start(|r| match r.method.as_str() {
        "POST" => Reply::Json(200, r#"{"id":"f","polling_url":"x"}"#.into()),
        _ if r.path.starts_with("/v1/get_result") => Reply::Json(
            200,
            json!({"id":"f","status":"Ready","result":{"sample":on_server(r, "/d.png")}})
                .to_string(),
        ),
        _ => http_bytes(200, &[], &result_png()),
    });
    run_cancel_paced(&provider(&ready.url), &model(), &fill_request(), &pause).unwrap();
}

#[test]
fn service_errors_keep_their_kind_and_never_the_key() {
    for (status, headers, kind) in [
        (
            429,
            vec![("Retry-After", "7".to_owned())],
            ErrorKind::RateLimited,
        ),
        (402, vec![], ErrorKind::Quota),
        (403, vec![], ErrorKind::Auth),
        (401, vec![], ErrorKind::Auth),
        (422, vec![], ErrorKind::InvalidRequest),
        (503, vec![], ErrorKind::Provider),
    ] {
        let h = headers.clone();
        let server = Server::start(move |_| {
            Reply::JsonWith(status, h.clone(), format!(r#"{{"detail":"{KEY}"}}"#))
        });
        let e = provider(&server.url)
            .submit(&model(), &fill_request())
            .unwrap_err();
        assert_eq!(e.kind, kind, "{status}");
        assert!(!e.message.contains(KEY));
        assert_eq!(e.retry_after_secs, (status == 429).then_some(7));
    }
    let e = provider(&Server::dead_url())
        .submit(&model(), &fill_request())
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Network);
}

fn finished_with(status: &'static str, sample: &'static str) -> (Server, JobState) {
    let server = Server::start(move |r| {
        match r.method.as_str() {
        "POST" => Reply::Json(200, r#"{"id":"m","polling_url":"x"}"#.into()),
        _ if r.path.starts_with("/v1/get_result") => Reply::Json(
            200,
            json!({"id":"m","status":status,"result":{"sample":sample.replace("HOST", r.header("host").unwrap())}})
                .to_string(),
        ),
        _ if r.path == "/expired.png" => Reply::Json(403, "{}".into()),
        _ => http_bytes(200, &[], b"<html>not a picture</html>"),
    }
    });
    let p = provider(&server.url);
    let id = p.submit(&model(), &fill_request()).unwrap();
    let state = p.poll(&id).unwrap().state;
    (server, state)
}

fn failure(state: &JobState) -> String {
    match state {
        JobState::Failed { error } => {
            assert_eq!(error.kind, ErrorKind::Provider);
            error.message.clone()
        }
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn moderation_bad_links_and_bad_results_fail_the_job() {
    let (_, s) = finished_with("Content Moderated", "");
    assert!(failure(&s).contains("moderated"));
    let (_, s) = finished_with("Request Moderated", "");
    assert!(failure(&s).contains("moderated"));
    let (server, s) = finished_with("Ready", "https://evil.example/x.png");
    assert!(failure(&s).contains("not a BFL delivery address"));
    assert_eq!(server.count(), 2, "the link was not followed");
    let (_, s) = finished_with("Ready", "http://HOST/expired.png");
    assert!(failure(&s).contains("expired"));
    let (_, s) = finished_with("Ready", "http://HOST/page.png");
    assert!(failure(&s).contains("not a PNG"));
}

#[test]
fn a_dropped_download_is_retried_on_the_next_poll() {
    let dead = Server::dead_url().replace("/v1", "/gone.png");
    let polls = Arc::new(AtomicU32::new(0));
    let server = Server::start(move |r| match r.method.as_str() {
        "POST" => Reply::Json(200, r#"{"id":"d","polling_url":"x"}"#.into()),
        _ if r.path.starts_with("/v1/get_result") => {
            let sample = if polls.fetch_add(1, Ordering::SeqCst) == 0 {
                dead.clone()
            } else {
                on_server(r, "/ok.png")
            };
            Reply::Json(
                200,
                json!({"id":"d","status":"Ready","result":{"sample":sample}}).to_string(),
            )
        }
        _ => http_bytes(200, &[], &result_png()),
    });
    let p = provider(&server.url);
    let id = p.submit(&model(), &fill_request()).unwrap();
    assert_eq!(p.poll(&id).unwrap_err().kind, ErrorKind::Network);
    let done = p.poll(&id).unwrap();
    assert_eq!(done.state, JobState::Succeeded);
    assert_eq!(done.outputs().count(), 1);
}

#[test]
fn only_official_and_regional_addresses_are_accepted() {
    for base in [
        "https://api.bfl.ai/v1",
        "https://api.eu.bfl.ai/v1",
        "https://api.us.bfl.ai/v1/",
    ] {
        assert!(
            BflProvider::new(BflConfig::new(base, SecretString::new("k"))).is_ok(),
            "{base}"
        );
    }
    for base in [
        "https://api.bfl.ai.evil.example/v1",
        "https://api.bfl.ml/v1",
    ] {
        assert!(BflProvider::new(BflConfig::new(base, SecretString::new("k"))).is_err());
    }
}
