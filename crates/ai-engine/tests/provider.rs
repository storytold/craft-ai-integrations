//! The provider around a runner: what a runner author never has to write.
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use craft_ai_core::{
    CapabilityId, Connection, ErrorKind, Event, JobId, JobState, ModelId, Provider, Request,
};
use craft_ai_engine::testing::{Behaviour, FakeRunner, check_runner};
use craft_ai_engine::{EngineProvider, EngineRegistry, ModelRunner};

fn weights(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let p = dir.path().join("m.safetensors");
    std::fs::write(&p, b"w").unwrap();
    p
}

fn fake() -> FakeRunner {
    FakeRunner::new("candle", "sam3", "image.segment")
}

fn model() -> ModelId {
    ModelId::new("sam3").unwrap()
}

fn wait(p: &dyn Provider, job: &JobId) -> (JobState, Vec<Event>) {
    let end = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    loop {
        let snap = p.poll(job).unwrap();
        events.extend(snap.events);
        if snap.state.is_terminal() {
            return (snap.state, events);
        }
        assert!(Instant::now() < end, "job never finished");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn provider(runner: FakeRunner, dir: &tempfile::TempDir) -> EngineProvider {
    EngineProvider::new(
        "candle",
        [(Arc::new(runner) as Arc<dyn ModelRunner>, weights(dir))],
    )
}

#[test]
fn a_good_runner_passes_the_shared_checks() {
    let dir = tempfile::tempdir().unwrap();
    let r = fake();
    let (good, bad) = (r.good_request().unwrap(), r.bad_request().unwrap());
    check_runner(
        Arc::new(r),
        &weights(&dir),
        &good,
        &bad,
        ErrorKind::InvalidRequest,
    )
    .unwrap();
}

#[test]
fn weights_load_once_across_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let r = fake();
    let loads = r.loads.clone();
    let req = r.good_request().unwrap();
    let p = provider(r, &dir);
    for _ in 0..3 {
        let job = p.submit(&model(), &req).unwrap();
        assert_eq!(wait(&p, &job).0, JobState::Succeeded);
    }
    assert_eq!(loads.load(Ordering::SeqCst), 1);
}

#[test]
fn progress_only_rises_and_output_arrives_once() {
    let dir = tempfile::tempdir().unwrap();
    let r = fake();
    let req = r.good_request().unwrap();
    let p = provider(r, &dir);
    let job = p.submit(&model(), &req).unwrap();
    let mut last = 0u8;
    let mut outputs = 0;
    loop {
        let snap = p.poll(&job).unwrap();
        outputs += snap.outputs().count();
        if let JobState::Running { percent: Some(n) } = snap.state {
            assert!(n >= last && n < 100);
            last = n;
        }
        if snap.state.is_terminal() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(outputs, 1);
    assert!(p.poll(&job).unwrap().events.is_empty());
}

#[test]
fn a_failing_runner_fails_the_job_with_its_words() {
    let dir = tempfile::tempdir().unwrap();
    let r = fake().behaving(Behaviour::Fail);
    let req = r.good_request().unwrap();
    let p = provider(r, &dir);
    let job = p.submit(&model(), &req).unwrap();
    match wait(&p, &job).0 {
        JobState::Failed { error } => assert_eq!(error.message, "the fake runner failed"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_panicking_runner_fails_the_job_and_is_loaded_afresh_next_time() {
    let dir = tempfile::tempdir().unwrap();
    let r = fake().behaving(Behaviour::Panic);
    let loads = r.loads.clone();
    let req = r.good_request().unwrap();
    let p = provider(r, &dir);
    for _ in 0..2 {
        let job = p.submit(&model(), &req).unwrap();
        match wait(&p, &job).0 {
            JobState::Failed { error } => assert_eq!(error.kind, ErrorKind::Internal),
            other => panic!("{other:?}"),
        }
    }
    // The model that panicked was dropped, not reused.
    assert_eq!(loads.load(Ordering::SeqCst), 2);
}

#[test]
fn cancel_ends_the_job_at_once_and_stays_ended() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = fake();
    r.steps = 200;
    let req = r.good_request().unwrap();
    let p = provider(r, &dir);
    let job = p.submit(&model(), &req).unwrap();
    p.cancel(&job).unwrap();
    assert_eq!(p.poll(&job).unwrap().state, JobState::Cancelled);
    std::thread::sleep(Duration::from_millis(60));
    let snap = p.poll(&job).unwrap();
    assert_eq!(snap.state, JobState::Cancelled);
    assert!(snap.events.is_empty());
}

#[test]
fn a_runner_that_ignores_cancel_cannot_bring_a_cancelled_job_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = fake();
    r.deaf = true;
    let r_loads = r.loads.clone();
    let req = r.good_request().unwrap();
    let p = provider(r, &dir);
    let loads = r_loads;
    let job = p.submit(&model(), &req).unwrap();
    while loads.load(Ordering::SeqCst) == 0 {
        std::thread::sleep(Duration::from_millis(1)); // it is running now
    }
    p.cancel(&job).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    let snap = p.poll(&job).unwrap();
    assert_eq!(snap.state, JobState::Cancelled);
    assert_eq!(snap.outputs().count(), 0);
}

#[test]
fn a_request_for_another_capability_is_refused_before_any_work() {
    let dir = tempfile::tempdir().unwrap();
    let r = fake();
    let loads = r.loads.clone();
    let p = provider(r, &dir);
    let other = Request::new(CapabilityId::parse("text.complete").unwrap());
    let e = p.submit(&model(), &other).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert_eq!(loads.load(Ordering::SeqCst), 0);
}

fn connection(provider: &str, model: &str, file: Option<&str>) -> Connection {
    let mut m = serde_json::json!({"id": model, "label": "SAM 3", "capabilities": []});
    if let Some(f) = file {
        m["local_file"] = f.into();
    }
    serde_json::from_value(serde_json::json!({
        "id": "c", "label": "SAM 3 built in", "provider": provider,
        "locality": "local", "auth": {"kind": "none"}, "billing": "free", "models": [m]
    }))
    .unwrap()
}

#[test]
fn a_connection_builds_when_its_engine_model_and_file_are_known() {
    let dir = tempfile::tempdir().unwrap();
    let w = weights(&dir);
    let mut registry = EngineRegistry::new();
    registry.register(fake());
    let c = connection("candle", "sam3", w.to_str());
    let p = EngineProvider::from_connection(&registry, &c).unwrap();
    assert_eq!(p.models().unwrap()[0].local_file.as_deref(), w.to_str());
}

#[test]
fn a_connection_that_cannot_run_says_why_in_plain_words() {
    let mut registry = EngineRegistry::new();
    registry.register(fake());
    let fails = |c: &Connection| EngineProvider::from_connection(&registry, c).err().unwrap();
    let e = fails(&connection("burn", "sam3", Some("x")));
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert!(e.message.contains("burn"));
    let e = fails(&connection("candle", "other", Some("x")));
    assert_eq!(e.kind, ErrorKind::Unsupported);
    let e = fails(&connection("candle", "sam3", None));
    assert_eq!(e.kind, ErrorKind::InvalidRequest);
    assert!(e.message.contains("Set up AI"));
}

#[test]
fn the_same_engine_and_model_replaces_and_engines_are_listed_once() {
    let mut registry = EngineRegistry::new();
    registry
        .register(fake())
        .register(fake())
        .register(FakeRunner::new("burn", "sam3", "image.segment"));
    assert_eq!(registry.engines(), vec!["candle", "burn"]);
    assert_eq!(registry.runners_of("CANDLE").len(), 1);
    assert!(registry.find("Candle", "sam3").is_some());
    assert!(registry.find("candle", "nope").is_none());
}

/// Waits until `check` is true, up to ten seconds.
fn until(what: &str, check: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < end, "never happened: {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_job_waiting_behind_another_says_queued_and_cancelled_it_never_runs() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = fake();
    r.steps = 1_000;
    let (loads, runs) = (r.loads.clone(), r.runs.clone());
    let req = r.good_request().unwrap();
    let p = provider(r, &dir);
    let first = p.submit(&model(), &req).unwrap();
    until("the first job runs", || runs.load(Ordering::SeqCst) == 1);
    let second = p.submit(&model(), &req).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(p.poll(&second).unwrap().state, JobState::Queued);
    p.cancel(&second).unwrap();
    // The first job is long enough to still be running here however slow this machine is; end it.
    p.cancel(&first).unwrap();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(p.poll(&second).unwrap().state, JobState::Cancelled);
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "the cancelled job must not run"
    );
    assert_eq!(loads.load(Ordering::SeqCst), 1);
}

#[test]
fn a_burst_of_requests_is_refused_in_plain_words_and_room_returns_when_they_end() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = fake();
    r.steps = 2_000;
    let req = r.good_request().unwrap();
    let p = provider(r, &dir);
    let mut jobs = Vec::new();
    let refused = loop {
        match p.submit(&model(), &req) {
            Ok(job) => jobs.push(job),
            Err(e) => break e,
        }
        assert!(jobs.len() < 100, "nothing ever said stop");
    };
    assert_eq!(refused.kind, ErrorKind::RateLimited);
    assert!(refused.message.contains("Wait for some to finish"));
    for job in &jobs {
        p.cancel(job).unwrap();
    }
    until("room again", || p.submit(&model(), &req).is_ok());
}
