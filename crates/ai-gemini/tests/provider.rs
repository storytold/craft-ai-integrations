//! The Gemini fill adapter against a scripted server on this computer: no real network, no real key.
#![allow(dead_code)]
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_core::conformance::{
    run_cancel_paced, run_lifecycle_paced, run_rejects, run_unknown_job,
};
use craft_ai_core::payload::{base64_decode, base64_encode};
use craft_ai_core::{
    CapabilityId, ErrorKind, Event, InputRole, JobState, ModelId, Payload, Provider, Request,
    SecretString,
};
use craft_ai_gemini::{DEFAULT_MODEL, GeminiConfig, GeminiProvider, OFFICIAL};
use serde_json::json;
use support::{Reply, Server};

const KEY: &str = "AIza-gemini-needle";

fn pause() {
    std::thread::sleep(std::time::Duration::from_millis(2));
}

fn png(
    width: u32,
    height: u32,
    color: png::ColorType,
    pixel: impl Fn(u32, u32) -> Vec<u8>,
) -> Vec<u8> {
    let data: Vec<u8> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .flat_map(|(x, y)| pixel(x, y))
        .collect();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&data).unwrap();
    writer.finish().unwrap();
    out
}

fn source() -> Vec<u8> {
    png(8, 8, png::ColorType::Rgb, |_, _| vec![10, 20, 30])
}

/// PhotoCraft's kind of mask: transparent and white where to fill.
fn mask() -> Vec<u8> {
    png(8, 8, png::ColorType::Rgba, |x, _| {
        if x < 4 {
            vec![255, 255, 255, 0]
        } else {
            vec![0, 0, 0, 255]
        }
    })
}

fn model() -> ModelId {
    ModelId::new(DEFAULT_MODEL).unwrap()
}

fn fill_request(prompt: &str) -> Request {
    Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", source()).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", mask()).unwrap(),
        )
        .with_input(InputRole::Prompt, Payload::text(prompt))
}

fn provider(url: &str) -> GeminiProvider {
    GeminiProvider::new(GeminiConfig::new(url, SecretString::new(KEY))).unwrap()
}

fn result_jpeg() -> Vec<u8> {
    b"\xff\xd8\xff\xe0 a jpeg".to_vec()
}

fn answer_server() -> Server {
    Server::start(|r| {
        assert_eq!(
            (r.method.as_str(), r.path.as_str()),
            ("POST", "/v1/interactions")
        );
        Reply::Json(
            200,
            json!({"steps": [{"type": "model_output", "content": [
                {"type": "text", "text": "Here you go"},
                {"type": "image", "mime_type": "image/jpeg", "data": base64_encode(&result_jpeg())}
            ]}]})
            .to_string(),
        )
    })
}

#[test]
fn a_fill_sends_the_mask_as_a_second_picture_with_the_key_only_in_a_header() {
    let server = answer_server();
    let p = provider(&server.url);
    let done = run_lifecycle_paced(&p, &model(), &fill_request("a red door"), &pause).unwrap();
    assert_eq!(done.state, JobState::Succeeded);
    let outputs: Vec<_> = done
        .events
        .iter()
        .filter_map(|e| match e {
            Event::Output(o) => Some(&o.payload),
            _ => None,
        })
        .collect();
    assert!(
        matches!(outputs.as_slice(), [Payload::Inline { media_type, data }] if media_type.as_str() == "image/jpeg" && *data == result_jpeg())
    );

    let sent = server.recorded();
    assert_eq!(sent.len(), 1);
    let r = &sent[0];
    assert!(!r.path.contains(KEY), "the key never goes in the URL");
    assert_eq!(r.header("x-goog-api-key"), Some(KEY));
    let body = r.body_json();
    assert_eq!(body["model"], DEFAULT_MODEL);
    let input = body["input"].as_array().unwrap();
    assert_eq!(input.len(), 3);
    let instruction = input[0]["text"].as_str().unwrap();
    assert!(
        instruction.contains("The second image is a mask"),
        "{instruction}"
    );
    assert!(
        instruction.contains("placeholder with: a red door."),
        "{instruction}"
    );
    assert_eq!(
        base64_decode(input[1]["data"].as_str().unwrap()).unwrap(),
        source()
    );
    // The mask is shown opaque, white where to fill, whatever form it arrived in.
    let shown = base64_decode(input[2]["data"].as_str().unwrap()).unwrap();
    let mut reader = png::Decoder::new(std::io::Cursor::new(shown))
        .read_info()
        .unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut buf).unwrap();
    assert_eq!(frame.color_type, png::ColorType::Grayscale);
    assert_eq!(&buf[..8], &[255, 255, 255, 255, 0, 0, 0, 0]);
    assert_eq!(body["response_format"]["aspect_ratio"], "1:1");
    assert_eq!(body["response_format"]["image_size"], "1K");
}

#[test]
fn an_empty_prompt_is_sent_as_a_removal() {
    let server = answer_server();
    let p = provider(&server.url);
    run_lifecycle_paced(&p, &model(), &fill_request(""), &pause).unwrap();
    let body = server.recorded()[0].body_json();
    assert!(
        body["input"][0]["text"]
            .as_str()
            .unwrap()
            .contains("had never existed")
    );
}

#[test]
fn testing_the_connection_lists_models_for_free_and_offers_the_image_models() {
    let server = Server::start(|r| {
        assert_eq!(
            (r.method.as_str(), r.path.as_str()),
            ("GET", "/v1/models?pageSize=1")
        );
        assert_eq!(r.header("x-goog-api-key"), Some(KEY));
        Reply::Json(
            200,
            r#"{"models":[{"name":"models/gemini-2.5-flash"}]}"#.into(),
        )
    });
    let models = provider(&server.url).models().unwrap();
    assert_eq!(models[0].id.as_str(), DEFAULT_MODEL);
    assert_eq!(models.len(), 4);
    let spec = models[0]
        .spec_for(&CapabilityId::parse("image.inpaint").unwrap())
        .unwrap();
    assert!(spec.requires.contains(&InputRole::Source) && spec.requires.contains(&InputRole::Mask));
    assert!(
        !spec.requires.contains(&InputRole::Prompt),
        "an empty prompt removes"
    );

    let bad_key = Server::start(|_| {
        Reply::Json(400, r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT"}}"#.into())
    });
    assert_eq!(
        provider(&bad_key.url).models().unwrap_err().kind,
        ErrorKind::Auth
    );
}

fn failure(reply: Reply) -> craft_ai_core::AiError {
    let server = Server::start(move |_| match &reply {
        Reply::Json(s, b) => Reply::Json(*s, b.clone()),
        Reply::JsonWith(s, h, b) => Reply::JsonWith(*s, h.clone(), b.clone()),
        _ => Reply::Json(500, "{}".into()),
    });
    let p = provider(&server.url);
    let done = run_lifecycle_paced(&p, &model(), &fill_request("x"), &pause).unwrap();
    match done.state {
        JobState::Failed { error } => error,
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn errors_map_to_kinds_and_never_carry_the_key() {
    let cases = [
        (Reply::Json(403, "{}".into()), ErrorKind::Auth),
        (
            Reply::Json(400, r#"{"error":{"message":"API key not valid.","status":"INVALID_ARGUMENT"}}"#.into()),
            ErrorKind::Auth,
        ),
        (
            Reply::Json(402, r#"[{"error":{"code":402,"message":"Your prepayment credits are depleted.","status":"RESOURCE_EXHAUSTED"}}]"#.into()),
            ErrorKind::Quota,
        ),
        (Reply::Json(400, r#"{"error":{"message":"Image too small"}}"#.into()), ErrorKind::InvalidRequest),
        (
            Reply::Json(500, format!(r#"{{"error":{{"message":"boom {KEY}"}}}}"#)),
            ErrorKind::Provider,
        ),
        (Reply::Json(200, r#"{"output_text":"I can't help with that."}"#.into()), ErrorKind::Provider),
        (Reply::Json(200, "{}".into()), ErrorKind::Provider),
        (Reply::Json(200, "<html>".into()), ErrorKind::Provider),
    ];
    for (reply, kind) in cases {
        let e = failure(reply);
        assert_eq!(e.kind, kind, "{e}");
        assert!(!format!("{e} {e:?}").contains(KEY), "{e}");
    }
    let e = failure(Reply::Json(
        402,
        r#"[{"error":{"message":"Your prepayment credits are depleted."}}]"#.into(),
    ));
    assert!(e.message.contains("prepayment credits"), "{e}");
    let e = failure(Reply::Json(
        200,
        r#"{"output_text":"I can't help with that."}"#.into(),
    ));
    assert!(e.message.contains("I can't help with that."), "{e}");
    let e = failure(Reply::JsonWith(
        429,
        vec![("Retry-After", "7".into())],
        "{}".into(),
    ));
    assert_eq!(
        (e.kind, e.retry_after_secs),
        (ErrorKind::RateLimited, Some(7))
    );
}

#[test]
fn a_dead_server_is_a_network_failure_of_the_job() {
    let p = provider(&Server::dead_url());
    let done = run_lifecycle_paced(&p, &model(), &fill_request("x"), &pause).unwrap();
    assert!(
        matches!(done.state, JobState::Failed { ref error } if error.kind == ErrorKind::Network)
    );
}

#[test]
fn cancel_and_unknown_jobs_behave() {
    let server = Server::start(|_| {
        std::thread::sleep(std::time::Duration::from_millis(300));
        Reply::Json(200, "{}".into())
    });
    let p = provider(&server.url);
    run_cancel_paced(&p, &model(), &fill_request("x"), &pause).unwrap();
    run_unknown_job(&p).unwrap();
}

#[test]
fn bad_requests_are_refused_before_anything_is_sent() {
    let server = Server::start(|_| Reply::Json(200, "{}".into()));
    let p = provider(&server.url);
    let generate = Request::new(CapabilityId::parse("image.generate").unwrap())
        .with_input(InputRole::Prompt, Payload::text("a fox"));
    run_rejects(&p, &model(), &generate, ErrorKind::Unsupported).unwrap();
    run_rejects(
        &p,
        &ModelId::new("gemini-2.5-pro").unwrap(),
        &fill_request("x"),
        ErrorKind::Unsupported,
    )
    .unwrap();
    run_rejects(
        &p,
        &model(),
        &fill_request("x").with_outputs(2),
        ErrorKind::Unsupported,
    )
    .unwrap();
    run_rejects(
        &p,
        &model(),
        &fill_request("x").streaming(),
        ErrorKind::Unsupported,
    )
    .unwrap();
    run_rejects(
        &p,
        &model(),
        &fill_request("x").with_param("seed", json!(3)),
        ErrorKind::Unsupported,
    )
    .unwrap();
    let no_mask = Request::new(CapabilityId::parse("image.inpaint").unwrap()).with_input(
        InputRole::Source,
        Payload::inline("image/png", source()).unwrap(),
    );
    run_rejects(&p, &model(), &no_mask, ErrorKind::InvalidRequest).unwrap();
    let wrong_size = Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", source()).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline(
                "image/png",
                png(4, 4, png::ColorType::Grayscale, |_, _| vec![255]),
            )
            .unwrap(),
        );
    run_rejects(&p, &model(), &wrong_size, ErrorKind::InvalidRequest).unwrap();
    assert_eq!(server.count(), 0);
}

#[test]
fn only_the_official_address_or_this_computer_is_accepted_and_debug_hides_the_key() {
    let make =
        |url: &str, key: &str| GeminiProvider::new(GeminiConfig::new(url, SecretString::new(key)));
    assert!(make(OFFICIAL, KEY).is_ok());
    assert!(make(&format!("{OFFICIAL}/"), KEY).is_ok());
    for url in [
        "https://aiplatform.googleapis.com/v1",
        "https://generativelanguage.googleapis.com.evil.example/v1beta",
        "http://generativelanguage.googleapis.com/v1beta",
        "https://generativelanguage.googleapis.com/v1beta?key=x",
    ] {
        assert_eq!(
            make(url, KEY).unwrap_err().kind,
            ErrorKind::InvalidRequest,
            "{url}"
        );
    }
    assert!(make(OFFICIAL, "two words").is_err());
    assert!(make(OFFICIAL, "").is_err());
    assert!(!format!("{:?}", make(OFFICIAL, KEY).unwrap()).contains(KEY));
}

#[test]
fn redirects_are_not_followed() {
    let server = Server::start(|_| {
        Reply::JsonWith(
            307,
            vec![("Location", "https://attacker.example".into())],
            "{}".into(),
        )
    });
    let p = provider(&server.url);
    let done = run_lifecycle_paced(&p, &model(), &fill_request("x"), &pause).unwrap();
    assert!(matches!(done.state, JobState::Failed { .. }));
    assert_eq!(server.count(), 1);
}
