//! Provider API calls are tried again where that is safe: a rate-limited send, and any poll that
//! failed for a reason that can pass. A dropped send is never repeated, because the job may
//! already be running and a second send could charge twice.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    cancels: AtomicUsize,
    /// How long each poll takes before it answers.
    poll_delay: Mutex<Duration>,
    /// When each poll began.
    poll_times: Mutex<Vec<Instant>>,
}

/// A mock provider that fails on cue.
struct Flaky {
    inner: MockProvider,
    faults: Arc<Faults>,
}

impl Provider for Flaky {
    /// The wrapped mock's models.
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        self.inner.models()
    }
    /// Counts the send, then hands back the next queued send error, or lets the mock start the job.
    fn submit(&self, model: &ModelId, request: &Request) -> Result<JobId, AiError> {
        self.faults.submits.fetch_add(1, Ordering::SeqCst);
        match self.faults.submit.lock().unwrap().pop_front() {
            Some(error) => Err(error),
            None => self.inner.submit(model, request),
        }
    }
    /// Counts the poll, waits the set delay, then hands back the next queued poll error or the mock's answer.
    fn poll(&self, job: &JobId) -> Result<JobSnapshot, AiError> {
        self.faults.polls.fetch_add(1, Ordering::SeqCst);
        self.faults.poll_times.lock().unwrap().push(Instant::now());
        std::thread::sleep(*self.faults.poll_delay.lock().unwrap());
        match self.faults.poll.lock().unwrap().pop_front().flatten() {
            Some(error) => Err(error),
            None => self.inner.poll(job),
        }
    }
    /// Counts the cancel and passes it to the mock.
    fn cancel(&self, job: &JobId) -> Result<(), AiError> {
        self.faults.cancels.fetch_add(1, Ordering::SeqCst);
        self.inner.cancel(job)
    }
}

/// The one text tool the tests run.
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

/// A prompt for that tool.
fn request() -> Request {
    Request::new(role().capability).with_input(InputRole::Prompt, Payload::text("hi"))
}

/// A rate limit: the provider refused the call.
fn busy() -> AiError {
    AiError::new(ErrorKind::RateLimited, "slow down")
}

/// A dropped connection.
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

/// A poll that keeps failing ends the wait once the tries run out, with the count, and the job
/// is cancelled rather than left running unwatched.
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
    assert_eq!(faults.cancels.load(Ordering::SeqCst), 1);
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

/// A client and a started job whose polls retry by `poll`.
fn started(faults: Arc<Faults>, poll: Backoff) -> (tempfile::TempDir, craft_ai_client::Job) {
    let (dir, client) = client(faults);
    let client = client.with_retries(QUICK, poll);
    let plan = client.plan(&role(), request()).unwrap();
    let job = client.start(&plan, &ConsentLedger::default()).unwrap();
    (dir, job)
}

/// The doubling pause is really taken between failed polls.
#[test]
fn the_retry_pause_is_taken() {
    let faults = Arc::new(Faults::default());
    faults
        .poll
        .lock()
        .unwrap()
        .extend([Some(dropped()), Some(dropped())]);
    let poll = Backoff {
        first_delay: Duration::from_millis(40),
        ..QUICK
    };
    let (_dir, job) = started(faults.clone(), poll);
    job.wait(Duration::ZERO, Duration::from_secs(5)).unwrap();
    let times = faults.poll_times.lock().unwrap();
    assert!(
        times[1] - times[0] >= Duration::from_millis(40),
        "{times:?}"
    );
    assert!(
        times[2] - times[1] >= Duration::from_millis(80),
        "{times:?}"
    );
}

/// A poll that says when to come back is not asked again sooner.
#[test]
fn a_polls_retry_after_is_honoured() {
    let faults = Arc::new(Faults::default());
    faults
        .poll
        .lock()
        .unwrap()
        .push_back(Some(busy().with_retry_after(1)));
    let (_dir, job) = started(faults.clone(), QUICK);
    job.wait(Duration::ZERO, Duration::from_secs(5)).unwrap();
    let times = faults.poll_times.lock().unwrap();
    assert!(times[1] - times[0] >= Duration::from_secs(1), "{times:?}");
}

/// A slow poll that fails (a timeout) does not use up the budget: it is still retried.
#[test]
fn a_slow_failed_poll_is_still_retried() {
    let faults = Arc::new(Faults::default());
    faults.poll.lock().unwrap().push_back(Some(dropped()));
    *faults.poll_delay.lock().unwrap() = Duration::from_millis(150);
    let poll = Backoff {
        max_attempts: 5,
        first_delay: Duration::ZERO,
        budget: Duration::from_millis(100),
    };
    let (_dir, job) = started(faults.clone(), poll);
    job.wait(Duration::ZERO, Duration::from_secs(5)).unwrap();
    assert_eq!(faults.polls.load(Ordering::SeqCst), 3);
}

/// A polling interval longer than the budget still retries a failed poll, and keeps the
/// interval as the least pause.
#[test]
fn a_long_interval_still_retries() {
    let faults = Arc::new(Faults::default());
    faults.poll.lock().unwrap().push_back(Some(dropped()));
    let poll = Backoff {
        budget: Duration::from_millis(100),
        ..QUICK
    };
    let (_dir, job) = started(faults.clone(), poll);
    job.wait(Duration::from_millis(300), Duration::from_secs(5))
        .unwrap();
    assert_eq!(faults.polls.load(Ordering::SeqCst), 3);
    let times = faults.poll_times.lock().unwrap();
    assert!(
        times[1] - times[0] >= Duration::from_millis(300),
        "{times:?}"
    );
}

/// The budget counts the pauses taken: one-second waits within a 2.5 s budget allow two
/// pauses, although each alone would fit, so the wait ends after three polls.
#[test]
fn the_budget_counts_the_pauses() {
    let faults = Arc::new(Faults::default());
    faults.poll.lock().unwrap().extend([
        Some(busy().with_retry_after(1)),
        Some(busy().with_retry_after(1)),
        Some(busy().with_retry_after(1)),
    ]);
    let poll = Backoff {
        max_attempts: 5,
        first_delay: Duration::ZERO,
        budget: Duration::from_millis(2500),
    };
    let (_dir, job) = started(faults.clone(), poll);
    let error = job
        .wait(Duration::ZERO, Duration::from_secs(10))
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::RateLimited);
    assert_eq!(faults.polls.load(Ordering::SeqCst), 3);
}

/// A pause that reaches the timeout ends the wait there: no further poll, and the job is
/// cancelled.
#[test]
fn no_poll_starts_after_the_timeout() {
    let faults = Arc::new(Faults::default());
    faults
        .poll
        .lock()
        .unwrap()
        .push_back(Some(busy().with_retry_after(1)));
    let (_dir, job) = started(faults.clone(), QUICK);
    let error = job
        .wait(Duration::ZERO, Duration::from_millis(200))
        .unwrap_err();
    assert_eq!(error.message, "the job took too long");
    assert_eq!(faults.polls.load(Ordering::SeqCst), 1);
    assert_eq!(faults.cancels.load(Ordering::SeqCst), 1);
}

/// A zero timeout asks nothing: the job is cancelled without a poll.
#[test]
fn a_zero_timeout_does_not_poll() {
    let faults = Arc::new(Faults::default());
    let (_dir, job) = started(faults.clone(), QUICK);
    let error = job.wait(Duration::ZERO, Duration::ZERO).unwrap_err();
    assert_eq!(error.message, "the job took too long");
    assert_eq!(faults.polls.load(Ordering::SeqCst), 0);
    assert_eq!(faults.cancels.load(Ordering::SeqCst), 1);
}

/// A failed poll that may not be retried and ends past the timeout is reported as a timeout,
/// not as the poll's own error, and the job is cancelled rather than left running.
#[test]
fn a_failure_past_the_timeout_cancels_the_job() {
    let faults = Arc::new(Faults::default());
    faults.poll.lock().unwrap().push_back(Some(dropped()));
    *faults.poll_delay.lock().unwrap() = Duration::from_millis(150);
    let (_dir, job) = started(faults.clone(), Backoff::NEVER);
    let error = job
        .wait(Duration::ZERO, Duration::from_millis(100))
        .unwrap_err();
    assert_eq!(error.message, "the job took too long");
    assert_eq!(faults.polls.load(Ordering::SeqCst), 1);
    assert_eq!(faults.cancels.load(Ordering::SeqCst), 1);
}
