//! Omni fill edge cases against the real adapter and a scripted server: a result that cannot be
//! fetched, a fill with no prompt, and a broken source picture.
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_artcraft::{ArtCraftConfig, ArtCraftProvider};
use craft_ai_core::{
    CapabilityId, ErrorKind, InputRole, JobState, ModelId, Payload, Provider, Request, SecretString,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use support::{Reply, Server};

/// An all-white RGBA PNG, so the same picture serves as source and as a mask that fills it all.
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

/// A fill request with `source` as the picture and an 8 × 8 mask, and the prompt if one is given.
fn fill(source: Vec<u8>, prompt: Option<&str>) -> Request {
    let request = Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", source).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", png(8, 8)).unwrap(),
        );
    match prompt {
        Some(text) => request.with_input(InputRole::Prompt, Payload::text(text)),
        None => request,
    }
}

/// The real adapter, allowed `active` jobs at once, pointed at the scripted server.
fn provider(server: &Server, active: usize) -> ArtCraftProvider {
    let mut config = ArtCraftConfig::new(
        server.url.trim_end_matches("/v1"),
        SecretString::new("fixture-only-key"),
    );
    config.max_active_jobs = active;
    ArtCraftProvider::new(config).unwrap()
}

/// A server whose uploads and generations succeed, whose job is finished, and whose result
/// address answers `result_status`. The prompt of each generation is kept in `prompts`.
fn finished_job_server(result_status: u16, prompts: Arc<std::sync::Mutex<Vec<String>>>) -> Server {
    let jobs = Arc::new(AtomicUsize::new(0));
    Server::start(move |r| match (r.method.as_str(), r.path.as_str()) {
        ("POST", "/v1/omni_api/upload/image") => {
            Reply::Json(200, r#"{"success":true,"media_file_token":"m_up"}"#.into())
        }
        ("POST", "/v1/omni_api/generate/image") => {
            let prompt = r.body_json()["prompt"].as_str().unwrap().to_owned();
            prompts.lock().unwrap().push(prompt);
            let n = jobs.fetch_add(1, Ordering::SeqCst);
            Reply::Json(
                200,
                format!(r#"{{"success":true,"inference_job_token":"jinf_{n}"}}"#),
            )
        }
        ("GET", path) if path.starts_with("/v1/omni_api/job_status/job/") => {
            let token = path.rsplit('/').next().unwrap();
            let uri = format!("http://{}/result.png", r.header("host").unwrap());
            Reply::Json(
                200,
                serde_json::json!({"success":true,"state":{"job_token":token,
                    "status":{"status":"complete_success"},
                    "maybe_result":{"media_links":{"cdn_url":uri}}}})
                .to_string(),
            )
        }
        ("GET", "/result.png") => Reply::Json(result_status, "{}".into()),
        _ => panic!("unexpected request {} {}", r.method, r.path),
    })
}

/// A busy result server is asked again on the next poll, but only a few times: then the job ends
/// as failed and its slot is free for the next job, without cancelling anything remotely.
#[test]
fn a_result_that_stays_unavailable_ends_the_job_and_frees_its_slot() {
    let server = finished_job_server(503, Arc::default());
    let p = provider(&server, 1);
    let model = ModelId::new("nano_banana").unwrap();
    let id = p.submit(&model, &fill(png(8, 8), Some("a door"))).unwrap();
    let busy = p.submit(&model, &fill(png(8, 8), Some("a door")));
    assert_eq!(
        busy.unwrap_err().kind,
        ErrorKind::Quota,
        "the one slot is taken"
    );
    for _ in 0..4 {
        assert_eq!(p.poll(&id).unwrap_err().kind, ErrorKind::Network);
    }
    let JobState::Failed { error } = p.poll(&id).unwrap().state else {
        panic!("the fifth failed fetch ends the job");
    };
    assert_eq!(error.kind, ErrorKind::Network);
    assert!(p.submit(&model, &fill(png(8, 8), Some("a door"))).is_ok());
}

/// A rate-limited result fetch is a passing failure too, with its own kind.
#[test]
fn a_rate_limited_result_is_tried_again() {
    let server = finished_job_server(429, Arc::default());
    let p = provider(&server, 4);
    let id = p
        .submit(
            &ModelId::new("nano_banana").unwrap(),
            &fill(png(8, 8), Some("a door")),
        )
        .unwrap();
    assert_eq!(p.poll(&id).unwrap_err().kind, ErrorKind::RateLimited);
}

/// Generative fill may leave the prompt out: the job is sent with the shared remove-what-is-there
/// instruction. Generation without words is still refused.
#[test]
fn a_fill_needs_no_prompt() {
    let prompts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let server = finished_job_server(503, prompts.clone());
    let p = provider(&server, 4);
    let model = ModelId::new("nano_banana").unwrap();
    p.submit(&model, &fill(png(8, 8), None)).unwrap();
    let sent = prompts.lock().unwrap().clone();
    assert_eq!(sent, vec![craft_ai_media::mask::fill_instruction("")]);
    let generate = Request::new(CapabilityId::parse("image.generate").unwrap())
        .with_input(InputRole::Prompt, Payload::text("  "));
    assert_eq!(
        p.submit(&model, &generate).unwrap_err().kind,
        ErrorKind::InvalidRequest
    );
}

/// A source whose header reads but whose pixels are cut off is refused before any upload.
#[test]
fn a_truncated_source_is_refused_before_upload() {
    let server = Server::start(|_| Reply::Json(500, "unexpected".into()));
    let p = provider(&server, 4);
    let whole = png(8, 8);
    let cut = whole[..whole.len() - 20].to_vec();
    let err = p
        .submit(
            &ModelId::new("nano_banana").unwrap(),
            &fill(cut, Some("a door")),
        )
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidRequest);
    assert_eq!(server.count(), 0);
}

/// A prompt that is not text is refused, not treated as a fill with no prompt.
#[test]
fn a_fill_with_a_non_text_prompt_is_refused() {
    let server = Server::start(|_| Reply::Json(500, "unexpected".into()));
    let p = provider(&server, 4);
    let request = fill(png(8, 8), None).with_input(
        InputRole::Prompt,
        Payload::inline("image/png", png(8, 8)).unwrap(),
    );
    let err = p
        .submit(&ModelId::new("nano_banana").unwrap(), &request)
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::InvalidRequest);
    assert_eq!(server.count(), 0);
}
