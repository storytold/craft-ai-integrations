//! Where BFL tasks are polled, and how long "Task not found" lasts. Scripted servers only.
#![allow(dead_code)]
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_bfl::{BflConfig, BflProvider};
use craft_ai_core::{
    CapabilityId, ErrorKind, InputRole, JobState, ModelId, Payload, Provider, Request, SecretString,
};
use serde_json::json;
use std::time::Duration;
use support::{Recorded, Reply, Server};

const KEY: &str = "poll-needle-key";

fn image() -> (ModelId, Request) {
    (
        ModelId::new("flux-3-image").unwrap(),
        Request::new(CapabilityId::parse("image.generate").unwrap())
            .with_input(InputRole::Prompt, Payload::text("a mountain")),
    )
}

fn provider(url: &str, grace: Duration) -> BflProvider {
    let mut config = BflConfig::new(url, SecretString::new(KEY));
    config.not_found_grace = grace;
    BflProvider::new(config).unwrap()
}

fn host(r: &Recorded) -> String {
    r.header("host").unwrap().to_owned()
}

fn ready(id: &str) -> Reply {
    Reply::Json(
        200,
        json!({"id":id,"status":"Ready","result":{"sample":"https://delivery.bfl.ai/r.png"}})
            .to_string(),
    )
}

#[test]
fn the_polling_url_from_the_submit_reply_is_used() {
    let server = Server::start(|r| {
        match (r.method.as_str(), r.path.as_str()) {
        ("POST", "/v1/flux-3-image") => Reply::Json(
            200,
            json!({"id":"t1","polling_url":format!("http://{}/v1/cluster-7/get_result?id=t1", host(r))})
                .to_string(),
        ),
        ("GET", "/v1/cluster-7/get_result?id=t1") => ready("t1"),
        _ => Reply::Json(404, "{}".into()),
    }
    });
    let p = provider(&server.url, Duration::from_secs(60));
    let (model, request) = image();
    let id = p.submit(&model, &request).unwrap();
    assert_eq!(p.poll(&id).unwrap().state, JobState::Succeeded);
    let polled: Vec<_> = server
        .recorded()
        .into_iter()
        .filter(|r| r.method == "GET")
        .collect();
    assert_eq!(polled.len(), 1);
    assert_eq!(polled[0].path, "/v1/cluster-7/get_result?id=t1");
    assert_eq!(polled[0].header("x-key"), Some(KEY));
}

#[test]
fn a_foreign_or_plain_http_polling_url_is_ignored_and_never_gets_the_key() {
    // A second server stands in for a foreign host: it must never be called.
    let foreign = Server::start(|_| Reply::Json(200, "{}".into()));
    let foreign_url = foreign.url.clone();
    for polling_url in [
        format!("{foreign_url}/get_result?id=t2"),
        "http://api.us1.bfl.ai/v1/get_result?id=t2".to_owned(),
        "https://api.bfl.ai.evil.example/v1/get_result?id=t2".to_owned(),
        "https://evil.example/v1/get_result?id=t2".to_owned(),
    ] {
        let reply = json!({"id":"t2","polling_url":polling_url}).to_string();
        let server = Server::start(move |r| match (r.method.as_str(), r.path.as_str()) {
            ("POST", "/v1/flux-3-image") => Reply::Json(200, reply.clone()),
            ("GET", "/v1/get_result?id=t2") => ready("t2"),
            _ => Reply::Json(404, "{}".into()),
        });
        let p = provider(&server.url, Duration::from_secs(60));
        let (model, request) = image();
        let id = p.submit(&model, &request).unwrap();
        assert_eq!(
            p.poll(&id).unwrap().state,
            JobState::Succeeded,
            "{polling_url}"
        );
        assert!(
            server
                .recorded()
                .iter()
                .any(|r| r.path == "/v1/get_result?id=t2")
        );
    }
    assert_eq!(foreign.count(), 0, "the key never went to a foreign host");
}

fn never_found(status_404: bool) -> Server {
    Server::start(move |r| match r.method.as_str() {
        "POST" => Reply::Json(200, r#"{"id":"gone","polling_url":"x"}"#.into()),
        _ if status_404 => Reply::Json(404, r#"{"detail":"Task not found"}"#.into()),
        _ => Reply::Json(200, r#"{"id":"gone","status":"Task not found"}"#.into()),
    })
}

#[test]
fn a_task_that_stays_not_found_fails_after_ten_polls_and_the_grace() {
    for status_404 in [false, true] {
        let server = never_found(status_404);
        let p = provider(&server.url, Duration::ZERO);
        let (model, request) = image();
        let id = p.submit(&model, &request).unwrap();
        for _ in 0..9 {
            assert_eq!(p.poll(&id).unwrap().state, JobState::Queued);
        }
        match p.poll(&id).unwrap().state {
            JobState::Failed { error } => {
                assert_eq!(error.kind, ErrorKind::Provider);
                assert!(error.message.contains("could not find the job"));
            }
            other => panic!("expected a failure, got {other:?}"),
        }
        assert!(matches!(
            p.poll(&id).unwrap().state,
            JobState::Failed { .. }
        ));
    }
}

#[test]
fn not_found_is_tolerated_within_the_grace_and_reset_by_any_other_answer() {
    // Within the grace, many polls do not fail the job.
    let server = never_found(false);
    let p = provider(&server.url, Duration::from_secs(3600));
    let (model, request) = image();
    let id = p.submit(&model, &request).unwrap();
    for _ in 0..25 {
        assert_eq!(p.poll(&id).unwrap().state, JobState::Queued);
    }
    // A found answer in between starts the count again.
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let server = Server::start(move |r| match r.method.as_str() {
        "POST" => Reply::Json(200, r#"{"id":"f","polling_url":"x"}"#.into()),
        _ => {
            let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let status = if n == 8 { "Pending" } else { "Task not found" };
            Reply::Json(200, json!({"id":"f","status":status}).to_string())
        }
    });
    let p = provider(&server.url, Duration::ZERO);
    let id = p.submit(&model, &request).unwrap();
    for _ in 0..18 {
        assert_eq!(p.poll(&id).unwrap().state, JobState::Queued);
    }
    assert!(matches!(
        p.poll(&id).unwrap().state,
        JobState::Failed { .. }
    ));
}
