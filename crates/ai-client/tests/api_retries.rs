//! Provider API calls are tried again where that is safe: a rate-limited send, and any poll that
//! failed for a reason that can pass. A dropped send is never repeated, because the job may
//! already be running and a second send could charge twice.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use craft_ai_client::{AiClient, ProviderFactory};
use craft_ai_core::mock::{MockProvider, Outcome, Script};
use craft_ai_core::{
    AiError, AppId, Backoff, Binding, CapabilityId, Connection, ConnectionId, ConsentLedger,
    ErrorKind, InputRole, JobId, JobSnapshot, JobState, ModelId, ModelInfo, Payload, Provider,
    Request, Role, RoleId, StoreFile,
};
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;
use serde_json::json;

/// Retries with no real pause, so the tests run at once.
const QUICK: Backoff = Backoff {
    max_attempts: 3,
    first_delay: Duration::ZERO,
    budget: Duration::from_secs(5),
};

/// Errors to hand back, in order, before the wrapped mock answers. A `None` in `poll` lets that
/// one poll through to the mock, so failures can sit between real answers.
#[derive(Default)]
struct Faults {
    submit: Mutex<VecDeque<AiError>>,
    poll: Mutex<VecDeque<Option<AiError>>>,
    submits: AtomicUsize,
    polls: AtomicUsize,
}

/// A mock provider that fails on cue.
struct Flaky {
    inner: MockProvider,
    faults: Arc<Faults>,
}

impl Provider for Flaky {
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        self.inner.models()
    }
    fn submit(&self, model: &ModelId, request: &Request) -> Result<JobId, AiError> {
        self.faults.submits.fetch_add(1, Ordering::SeqCst);
        match self.faults.submit.lock().unwrap().pop_front() {
            Some(error) => Err(error),
            None => self.inner.submit(model, request),
        }
    }
    fn poll(&self, job: &JobId) -> Result<JobSnapshot, AiError> {
        self.faults.polls.fetch_add(1, Ordering::SeqCst);
        match self.faults.poll.lock().unwrap().pop_front().flatten() {
            Some(error) => Err(error),
            None => self.inner.poll(job),
        }
    }
    fn cancel(&self, job: &JobId) -> Result<(), AiError> {
        self.inner.cancel(job)
    }
}

fn role() -> Role {
    Role {
        id: RoleId::new("rewrite").unwrap(),
        label: "Rewrite".into(),
        capability: CapabilityId::parse("text.complete").unwrap(),
        inputs: vec![InputRole::Prompt],
    }
}

/// A client whose one local connection runs the flaky provider, with quick retries.
fn client(faults: Arc<Faults>) -> (tempfile::TempDir, AiClient) {
    let dir = tempfile::tempdir().unwrap();
    let models: Vec<_> = craft_ai_openai::catalog::classify("llama3")
        .into_iter()
        .collect();
    let connection: Connection = serde_json::from_value(json!({
        "id": "here", "label": "Here", "provider": "flaky",
        "endpoint": "http://127.0.0.1:9/v1", "locality": "local",
        "auth": {"kind": "none"}, "billing": "free", "models": models,
    }))
    .unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    shared
        .replace(&StoreFile {
            connections: vec![connection],
            defaults: vec![Binding {
                role: role().id,
                connection: ConnectionId::new("here").unwrap(),
                model: ModelId::new("llama3").unwrap(),
            }],
            ..StoreFile::default()
        })
        .unwrap();
    let mut factory = ProviderFactory::standard();
    factory.register("flaky", move |conn, _| {
        let script = Script {
            running_polls: 1,
            text_chunks: Vec::new(),
            outcome: Outcome::Succeed(Vec::new()),
        };
        Ok(Arc::new(Flaky {
            inner: MockProvider::new(conn.models.clone(), script),
            faults: faults.clone(),
        }) as Arc<dyn Provider>)
    });
    let app = AppId::new("wordcraft").unwrap();
    let client = AiClient::new(app, shared, Arc::new(MemoryStore::default()))
        .with_factory(factory)
        .with_retries(QUICK, QUICK);
    (dir, client)
}

fn request() -> Request {
    Request::new(role().capability).with_input(InputRole::Prompt, Payload::text("hi"))
}

fn busy() -> AiError {
    AiError::new(ErrorKind::RateLimited, "slow down")
}

fn dropped() -> AiError {
    AiError::new(ErrorKind::Network, "could not reach the provider")
}

/// A rate-limited send is tried again and the job starts.
#[test]
fn a_rate_limited_send_is_tried_again() {
    let faults = Arc::new(Faults::default());
    faults.submit.lock().unwrap().extend([busy(), busy()]);
    let (_dir, client) = client(faults.clone());
    let plan = client.plan(&role(), request()).unwrap();
    let job = client.start(&plan, &ConsentLedger::default()).unwrap();
    assert_eq!(faults.submits.load(Ordering::SeqCst), 3);
    let done = job.wait(Duration::ZERO, Duration::from_secs(5)).unwrap();
    assert_eq!(done.state, JobState::Succeeded);
}

/// A dropped send is reported at once and never sent twice.
#[test]
fn a_dropped_send_is_not_sent_again() {
    let faults = Arc::new(Faults::default());
    faults.submit.lock().unwrap().push_back(dropped());
    let (_dir, client) = client(faults.clone());
    let plan = client.plan(&role(), request()).unwrap();
    let error = client.start(&plan, &ConsentLedger::default()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Network);
    assert_eq!(faults.submits.load(Ordering::SeqCst), 1);
    assert!(!error.message.contains("tries"));
}

/// A rate limit that outlasts the tries is reported with its kind and the count.
#[test]
fn a_send_that_stays_rate_limited_says_how_many_tries() {
    let faults = Arc::new(Faults::default());
    faults
        .submit
        .lock()
        .unwrap()
        .extend([busy(), busy(), busy()]);
    let (_dir, client) = client(faults.clone());
    let plan = client.plan(&role(), request()).unwrap();
    let error = client.start(&plan, &ConsentLedger::default()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::RateLimited);
    assert_eq!(faults.submits.load(Ordering::SeqCst), 3);
    assert!(
        error.message.ends_with("(after 3 tries)"),
        "{}",
        error.message
    );
}

/// Polls that drop or are rate limited do not end the wait; the job still finishes.
#[test]
fn a_failed_poll_does_not_end_the_wait() {
    let faults = Arc::new(Faults::default());
    faults
        .poll
        .lock()
        .unwrap()
        .extend([Some(dropped()), Some(busy())]);
    let (_dir, client) = client(faults.clone());
    let plan = client.plan(&role(), request()).unwrap();
    let job = client.start(&plan, &ConsentLedger::default()).unwrap();
    let done = job.wait(Duration::ZERO, Duration::from_secs(5)).unwrap();
    assert_eq!(done.state, JobState::Succeeded);
    assert!(faults.polls.load(Ordering::SeqCst) >= 4);
}

/// A success resets the run of failures, so scattered blips never add up to a give-up. Four
/// failures in all, never three in a row: with three tries allowed, only a reset gets through.
#[test]
fn a_success_resets_the_failure_run() {
    let faults = Arc::new(Faults::default());
    faults.poll.lock().unwrap().extend([
        Some(dropped()),
        Some(dropped()),
        None,
        Some(dropped()),
        Some(dropped()),
    ]);
    let (_dir, client) = client(faults.clone());
    let plan = client.plan(&role(), request()).unwrap();
    let job = client.start(&plan, &ConsentLedger::default()).unwrap();
    let done = job.wait(Duration::ZERO, Duration::from_secs(5)).unwrap();
    assert_eq!(done.state, JobState::Succeeded);
    assert_eq!(faults.polls.load(Ordering::SeqCst), 6);
}

/// A poll that keeps failing ends the wait once the tries run out, with the count.
#[test]
fn a_poll_that_keeps_failing_ends_the_wait() {
    let faults = Arc::new(Faults::default());
    faults
        .poll
        .lock()
        .unwrap()
        .extend([Some(dropped()), Some(dropped()), Some(dropped())]);
    let (_dir, client) = client(faults.clone());
    let plan = client.plan(&role(), request()).unwrap();
    let job = client.start(&plan, &ConsentLedger::default()).unwrap();
    let error = job
        .wait(Duration::ZERO, Duration::from_secs(5))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Network);
    assert!(
        error.message.ends_with("(after 3 tries)"),
        "{}",
        error.message
    );
    assert_eq!(faults.polls.load(Ordering::SeqCst), 3);
}

/// A poll error that cannot pass (the provider refused the key) ends the wait at once.
#[test]
fn a_poll_refusal_is_not_retried() {
    let faults = Arc::new(Faults::default());
    faults
        .poll
        .lock()
        .unwrap()
        .push_back(Some(AiError::new(ErrorKind::Auth, "key refused")));
    let (_dir, client) = client(faults.clone());
    let plan = client.plan(&role(), request()).unwrap();
    let job = client.start(&plan, &ConsentLedger::default()).unwrap();
    let error = job
        .wait(Duration::ZERO, Duration::from_secs(5))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Auth);
    assert_eq!(faults.polls.load(Ordering::SeqCst), 1);
}

/// NEVER turns poll retries off: the first failed poll ends the wait, as before.
#[test]
fn retries_can_be_turned_off() {
    let faults = Arc::new(Faults::default());
    faults.poll.lock().unwrap().push_back(Some(dropped()));
    faults.submit.lock().unwrap().push_back(busy());
    let (_dir, client) = client(faults.clone());
    let client = client.with_retries(Backoff::NEVER, Backoff::NEVER);
    let plan = client.plan(&role(), request()).unwrap();
    let error = client.start(&plan, &ConsentLedger::default()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::RateLimited);
    let job = client.start(&plan, &ConsentLedger::default()).unwrap();
    let error = job
        .wait(Duration::ZERO, Duration::from_secs(5))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Network);
    assert_eq!(faults.polls.load(Ordering::SeqCst), 1);
}
