//! A reply's cost once parsed is bounded, not only its bytes. A reply of many tiny objects costs
//! about a hundred times its size as a plain `serde_json::Value`; each reader refuses it within
//! its budget, while real replies of the same or larger size still succeed. Scripted localhost
//! servers only.

mod support;

use craft_ai_core::conformance::run_lifecycle_paced;
use craft_ai_core::job::{Event, JobState};
use craft_ai_core::payload::base64_encode;
use craft_ai_core::{
    CapabilityId, ErrorKind, InputRole, ModelId, Payload, Provider, Request, SecretString,
};
use craft_ai_openai::probe::list_models;
use craft_ai_openai::{OpenAiConfig, OpenAiProvider};
use serde_json::json;
use support::{Reply, Server, sse};

/// `n` tiny objects, comma-separated: about 7 bytes each on the wire, about 800 once parsed.
fn tiny_objects(n: usize) -> String {
    vec![r#"{"":0}"#; n].join(",")
}

/// About 1 MiB of tiny objects: some 120 MiB as a plain `Value`.
const TINY: usize = 150_000;

fn config(url: &str) -> OpenAiConfig {
    let mut config = OpenAiConfig::new(url).with_key(SecretString::new("sk-size-test"));
    config.send_retry = craft_ai_core::Backoff::NEVER;
    config
}

fn provider(url: &str) -> OpenAiProvider {
    OpenAiProvider::new(config(url)).unwrap()
}

fn run(url: &str, model: &str, request: &Request) -> craft_ai_core::conformance::Completed {
    let pause = || std::thread::sleep(std::time::Duration::from_millis(5));
    run_lifecycle_paced(
        &provider(url),
        &ModelId::new(model).unwrap(),
        request,
        &pause,
    )
    .unwrap()
}

fn chat() -> Request {
    Request::new(CapabilityId::parse("text.complete").unwrap())
        .with_input(InputRole::Prompt, Payload::text("hi"))
}

fn image() -> Request {
    Request::new(CapabilityId::parse("image.generate").unwrap())
        .with_input(InputRole::Prompt, Payload::text("a fox"))
}

fn assert_refused_as_too_large(state: &JobState) {
    match state {
        JobState::Failed { error } => {
            assert_eq!(error.kind, ErrorKind::Provider);
            assert!(error.message.contains("too large"), "{}", error.message);
        }
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn a_model_list_of_tiny_objects_is_refused() {
    let body = format!(r#"{{"data":[{}]}}"#, tiny_objects(TINY));
    let server = Server::start(move |_| Reply::Json(200, body.clone()));
    let e = provider(&server.url).models().unwrap_err();
    assert_eq!(e.kind, ErrorKind::Provider);
    assert!(e.message.contains("too large"), "{}", e.message);
    // The connection's Test button reads the same list.
    let e = list_models(&config(&server.url)).unwrap_err();
    assert!(e.message.contains("too large"), "{}", e.message);
}

#[test]
fn a_model_list_has_a_byte_cap_of_its_own() {
    // 9 MiB of plain text in one field: cheap to parse, but no model list is that long.
    let padding = "x".repeat(9 * 1024 * 1024);
    let body = json!({"data":[{"id":"gpt-4o","note":padding}]}).to_string();
    let server = Server::start(move |_| Reply::Json(200, body.clone()));
    let e = list_models(&config(&server.url)).unwrap_err();
    assert!(e.message.contains("too large"), "{}", e.message);
}

#[test]
fn a_real_sized_model_list_still_reads() {
    let data: Vec<_> = (0..2000)
        .map(|i| {
            json!({"id": format!("gpt-4o-{i:04}"), "object": "model", "created": 1,
                         "owned_by": "someone", "pricing": {"prompt": "0.000001"}})
        })
        .collect();
    let body = json!({ "data": data }).to_string();
    let server = Server::start(move |_| Reply::Json(200, body.clone()));
    assert_eq!(provider(&server.url).models().unwrap().len(), 2000);
}

#[test]
fn a_chat_reply_of_tiny_objects_is_refused() {
    let body = format!(
        r#"{{"choices":[{{"message":{{"content":"hello"}}}}],"extra":[{}]}}"#,
        tiny_objects(TINY)
    );
    let server = Server::start(move |_| Reply::Json(200, body.clone()));
    assert_refused_as_too_large(&run(&server.url, "gpt-4o", &chat()).state);
}

#[test]
fn structured_output_of_tiny_objects_is_refused() {
    // The reply itself is one short string; the model's JSON inside it is what is re-read.
    let text = format!("[{}]", tiny_objects(TINY));
    let body = json!({"choices":[{"message":{"content": text}}]}).to_string();
    let server = Server::start(move |_| Reply::Json(200, body.clone()));
    let request = chat().with_param("response_format", json!({"type": "json_object"}));
    assert_refused_as_too_large(&run(&server.url, "gpt-4o", &request).state);
}

#[test]
fn a_stream_line_of_tiny_objects_is_refused() {
    let line = format!(
        r#"{{"choices":[{{"delta":{{"content":"hi"}}}}],"extra":[{}]}}"#,
        tiny_objects(TINY / 2)
    );
    let server = Server::start(move |_| Reply::Sse(vec![sse(&line), sse("[DONE]")]));
    let request = chat().streaming();
    assert_refused_as_too_large(&run(&server.url, "gpt-4o", &request).state);
}

#[test]
fn an_image_reply_of_tiny_objects_is_refused() {
    // An images reply has the largest budget (room for several pictures), so it takes about
    // 2 MiB of tiny objects, some 240 MiB as a plain `Value`, to pass it.
    let body = format!(
        r#"{{"data":[{{"url":"https://example.com/a.png"}}],"extra":[{}]}}"#,
        tiny_objects(2 * TINY)
    );
    let server = Server::start(move |_| Reply::Json(200, body.clone()));
    assert_refused_as_too_large(&run(&server.url, "gpt-image-1", &image()).state);
}

#[test]
fn a_large_picture_still_arrives() {
    // One 20 MiB base64 picture: the large reply an image server really sends.
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    png.resize(16 * 1024 * 1024, 0);
    let b64 = base64_encode(&png);
    assert!(b64.len() > 20 * 1024 * 1024);
    let body = json!({"data":[{"b64_json": b64}]}).to_string();
    let server = Server::start(move |_| Reply::Json(200, body.clone()));
    let done = run(&server.url, "gpt-image-1", &image());
    assert_eq!(done.state, JobState::Succeeded);
    assert!(matches!(done.events.first(),
        Some(Event::Output(o)) if matches!(&o.payload, Payload::Inline { data, .. } if data.len() == png.len())));
}
