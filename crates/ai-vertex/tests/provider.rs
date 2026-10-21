#![allow(dead_code)]
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;
use craft_ai_core::{
    ErrorKind, InputRole, JobState, ModelId, Payload, Provider, Request, SecretString,
    capability::CapabilityId,
};
use craft_ai_vertex::{VertexConfig, VertexProvider};
use serde_json::json;
use support::{Reply, Server};

/// One express-mode request sends the key only in the header and returns the inline image.
#[test]
fn image_generation_uses_express_key_and_returns_inline_image() {
    let s = Server::start(|r| {
        assert_eq!(r.method, "POST");
        // The key must never appear in the URL: request lines are what TLS-intercepting
        // proxies and access logs record, and SECURITY.md lists a key reaching a log as
        // reportable. It travels in the x-goog-api-key header instead.
        assert_eq!(
            r.path,
            "/v1/publishers/google/models/gemini-2.5-flash-image:generateContent"
        );
        assert!(!r.path.contains("test-key"));
        assert_eq!(r.header("x-goog-api-key"), Some("test-key"));
        assert_eq!(
            r.body_json(),
            json!({"contents":[{"role":"user","parts":[{"text":"a fox"}]}],"generationConfig":{"responseModalities":["TEXT","IMAGE"]}})
        );
        Reply::Json(200, r#"{"candidates":[{"content":{"parts":[{"text":"done"},{"inlineData":{"mimeType":"image/png","data":"AQID"}}]}}]}"#.into())
    });
    let p = VertexProvider::new(VertexConfig::new(
        s.url.clone(),
        SecretString::new("test-key"),
    ))
    .unwrap();
    let request = Request::new(CapabilityId::parse("image.generate").unwrap())
        .with_input(InputRole::Prompt, Payload::text("a fox"));
    let job = p
        .submit(&ModelId::new("gemini-2.5-flash-image").unwrap(), &request)
        .unwrap();
    let done = p.poll(&job).unwrap();
    assert_eq!(done.state, JobState::Succeeded);
    assert!(
        matches!(&done.outputs().next().unwrap().payload, Payload::Inline { data, .. } if data == &[1,2,3])
    );
    assert!(p.poll(&job).unwrap().events.is_empty());
}

/// Other models and unsupported options are refused before any call leaves the machine.
#[test]
fn submit_refuses_other_models_and_options_before_any_call() {
    // A wrong model and every unsupported option are refused on the caller's thread,
    // before a request could leave: the server is never contacted (count stays 0).
    let s = Server::start(|_| Reply::Json(200, "{}".into()));
    let p = VertexProvider::new(VertexConfig::new(
        s.url.clone(),
        SecretString::new("needle-secret"),
    ))
    .unwrap();
    let model = ModelId::new("gemini-2.5-flash-image").unwrap();
    let plain = Request::new(CapabilityId::parse("image.generate").unwrap())
        .with_input(InputRole::Prompt, Payload::text("a fox"));
    let cases = [
        (ModelId::new("gemini-2.0-flash").unwrap(), plain.clone()),
        (model.clone(), plain.clone().streaming()),
        (model.clone(), plain.clone().with_outputs(2)),
        (model, plain.with_param("temperature", json!(0.5))),
    ];
    for (m, r) in cases {
        assert_eq!(p.submit(&m, &r).unwrap_err().kind, ErrorKind::Unsupported);
        assert_eq!(s.count(), 0);
    }
}

/// Bad requests never reach the network, redirects are not followed, and Debug keeps the key hidden.
#[test]
fn refuses_unsupported_before_paid_call_and_never_follows_redirects() {
    let s = Server::start(|_| {
        Reply::JsonWith(
            307,
            vec![("Location", "https://attacker.example".into())],
            "{}".into(),
        )
    });
    let p = VertexProvider::new(VertexConfig::new(
        s.url.clone(),
        SecretString::new("needle-secret"),
    ))
    .unwrap();
    let request = Request::new(CapabilityId::parse("image.generate").unwrap())
        .with_input(InputRole::Prompt, Payload::text("a fox"))
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", vec![1]).unwrap(),
        );
    assert_eq!(
        p.submit(&ModelId::new("gemini-2.5-flash-image").unwrap(), &request)
            .unwrap_err()
            .kind,
        ErrorKind::Unsupported
    );
    assert_eq!(s.count(), 0);
    assert!(!format!("{p:?}").contains("needle-secret"));
    let e = p.models().unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert_eq!(s.count(), 0);
}
