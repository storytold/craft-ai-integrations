//! `AppTools` when a job does not end cleanly, and what a text tool returns.
//!
//! Scripted stand-in only: a temporary store, an in-memory key store and a stand-in service that
//! answers with text, refuses to stop, sends an answer that cannot be read, or counts how often it
//! is asked. No network and no model.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use craft_ai_client::{AiClient, AppTools, ProviderFactory, RoleStatus};
use craft_ai_core::{
    AiError, AppId, Binding, CapabilityId, CapabilitySpec, Connection, ConnectionId, ConsentLedger,
    Event, InputRole, JobId, JobSnapshot, JobState, ModelId, ModelInfo, Payload, Provider, Request,
    Role, RoleId, SecretRef, SecretString, StoreFile,
};
use craft_ai_native::secrets::{MemoryStore, SecretStore};
use craft_ai_native::shared::SharedStore;
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// What the stand-in service does with a job.
#[derive(Clone, Copy)]
enum Mode {
    /// Finishes at once with a streamed text reply and no outputs.
    Text,
    /// Runs for ever and refuses to stop.
    Refuses,
    /// Sends an answer that cannot be read (an error that is not worth trying again).
    Broken,
    /// Finishes after `FINISH`, counting every look.
    Counting,
}

/// How long a `Counting` job runs.
const FINISH: Duration = Duration::from_millis(2500);

struct Stub {
    mode: Mode,
    started: Instant,
    polls: Arc<AtomicUsize>,
    cancels: Arc<AtomicUsize>,
}

fn info() -> ModelInfo {
    let capability = CapabilityId::parse("text.complete").unwrap();
    ModelInfo::new(ModelId::new("stub-model").unwrap(), "Stub").with(
        CapabilitySpec::new(capability)
            .accepting(InputRole::Prompt, 1)
            .requiring(InputRole::Prompt),
    )
}

impl Provider for Stub {
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        Ok(vec![info()])
    }
    fn submit(&self, _: &ModelId, _: &Request) -> Result<JobId, AiError> {
        Ok(JobId::new("stub-1").unwrap())
    }
    fn poll(&self, _: &JobId) -> Result<JobSnapshot, AiError> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        let running = JobSnapshot::new(JobState::Running { percent: None });
        match self.mode {
            Mode::Text => Ok(JobSnapshot {
                state: JobState::Succeeded,
                events: vec![
                    Event::TextDelta {
                        text: "Hello, ".to_owned(),
                    },
                    Event::TextDelta {
                        text: "world".to_owned(),
                    },
                ],
                usage: None,
            }),
            Mode::Refuses => Ok(running),
            Mode::Broken => Err(AiError::invalid("an answer this app cannot read")),
            Mode::Counting if self.started.elapsed() >= FINISH => {
                Ok(JobSnapshot::new(JobState::Succeeded))
            }
            Mode::Counting => Ok(running),
        }
    }
    fn cancel(&self, _: &JobId) -> Result<(), AiError> {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        match self.mode {
            Mode::Refuses => Err(AiError::unsupported("this service cannot stop a job")),
            _ => Ok(()),
        }
    }
}

/// The custom text tool the store binds to the stand-in.
fn role() -> Role {
    Role {
        id: RoleId::new("rewrite").unwrap(),
        label: "Rewrite".to_owned(),
        capability: CapabilityId::parse("text.complete").unwrap(),
        inputs: vec![InputRole::Prompt],
    }
}

/// A stand-in world: the store, the tools and the stand-in's counters.
struct World {
    _dir: tempfile::TempDir,
    tools: AppTools,
    polls: Arc<AtomicUsize>,
    cancels: Arc<AtomicUsize>,
}

/// `AppTools` for an app whose `rewrite` tool is bound to the stand-in, on this computer or
/// (`remote`) at an online service with a saved key.
fn world(mode: Mode, remote: bool) -> Result<World, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let mut connection: Connection = serde_json::from_value(json!({
        "id": "stub", "label": "Stub", "provider": "stub",
        "endpoint": if remote { "https://api.example.com/v1" } else { "http://127.0.0.1:9" },
        "locality": if remote { "remote" } else { "local" },
        "auth": if remote { json!({"kind": "api_key", "secret": "stub-key"}) } else { json!({"kind": "none"}) },
        "billing": if remote { "metered" } else { "free" },
        "models": [],
    }))?;
    connection.models = vec![info()];
    shared.replace(&StoreFile {
        connections: vec![connection],
        defaults: vec![Binding {
            role: role().id,
            connection: ConnectionId::new("stub")?,
            model: ModelId::new("stub-model")?,
        }],
        custom_tools: vec![role()],
        ..StoreFile::default()
    })?;
    let secrets = Arc::new(MemoryStore::new());
    secrets.set(&SecretRef::new("stub-key")?, &SecretString::new("k"))?;
    let (polls, cancels) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let mut factory = ProviderFactory::standard();
    let (p, c) = (polls.clone(), cancels.clone());
    factory.register("stub", move |_, _| {
        Ok(Arc::new(Stub {
            mode,
            started: Instant::now(),
            polls: p.clone(),
            cancels: c.clone(),
        }) as Arc<dyn Provider>)
    });
    let client = AiClient::new(AppId::new("wordcraft")?, shared, secrets).with_factory(factory);
    Ok(World {
        _dir: dir,
        tools: AppTools::with_client(client),
        polls,
        cancels,
    })
}

/// Plans `rewrite` with a prompt and agrees to what it sends.
fn planned(tools: &AppTools) -> Result<(craft_ai_client::Plan, ConsentLedger), String> {
    let request = tools
        .request("rewrite")
        .ok_or("no rewrite tool")?
        .with_input(InputRole::Prompt, Payload::text("tidy this"));
    let plan = tools.plan("rewrite", request)?;
    let mut ledger = ConsentLedger::default();
    ledger.grant(plan.disclosure());
    Ok((plan, ledger))
}

/// A text tool's reply is returned as one text output, not lost because it arrived as text.
#[test]
fn a_text_reply_is_returned_as_one_text_output() -> TestResult {
    let w = world(Mode::Text, false)?;
    let (plan, ledger) = planned(&w.tools)?;
    let outputs = w.tools.run(&plan, &ledger)?;
    assert_eq!(outputs.len(), 1, "{outputs:?}");
    assert_eq!(outputs[0].payload, Payload::text("Hello, world"));
    Ok(())
}

/// A job that will not stop at the end of the wait is reported as possibly still running.
#[test]
fn a_job_that_will_not_stop_is_reported_as_still_running() -> TestResult {
    let w = world(Mode::Refuses, false)?;
    let (plan, ledger) = planned(&w.tools)?;
    let error = w
        .tools
        .run_for(&plan, &ledger, Duration::from_millis(100))
        .unwrap_err();
    assert!(error.contains("may still be running"), "{error}");
    Ok(())
}

/// A cancel that a job refuses says so, instead of reporting that the job stopped.
#[test]
fn a_refused_cancel_is_reported() -> TestResult {
    let w = world(Mode::Refuses, false)?;
    let tools = Arc::new(w.tools);
    let canceller = tools.canceller();
    assert_eq!(
        canceller.cancel(),
        Ok(()),
        "nothing running is not an error"
    );
    let (plan, ledger) = planned(&tools)?;
    let waiting = {
        let tools = tools.clone();
        std::thread::spawn(move || tools.run_for(&plan, &ledger, Duration::from_secs(2)))
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while canceller.waiting() == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let refused = canceller.cancel().unwrap_err();
    assert!(refused.contains("may still be running"), "{refused}");
    let _ = waiting.join();
    Ok(())
}

/// An answer that cannot be read ends the wait, and the job is asked to stop rather than left
/// running at the service.
#[test]
fn an_unreadable_answer_asks_the_job_to_stop() -> TestResult {
    let w = world(Mode::Broken, false)?;
    let (plan, ledger) = planned(&w.tools)?;
    let error = w.tools.run(&plan, &ledger).unwrap_err();
    assert!(error.contains("cannot read"), "{error}");
    assert!(!error.contains("may still be running"), "{error}");
    assert!(
        w.cancels.load(Ordering::SeqCst) >= 1,
        "the job was not asked to stop"
    );
    Ok(())
}

/// A job at an online service is looked at about once a second, not many times a second.
#[test]
fn an_online_job_is_not_asked_many_times_a_second() -> TestResult {
    let w = world(Mode::Counting, true)?;
    let (plan, ledger) = planned(&w.tools)?;
    w.tools.run(&plan, &ledger)?;
    let polls = w.polls.load(Ordering::SeqCst);
    assert!(polls <= 5, "asked {polls} times in 2.5 seconds");
    Ok(())
}

/// A store that cannot be read is not mistaken for a tool that does not exist: the store's reason
/// is shown.
#[test]
fn an_unreadable_store_is_not_an_unknown_tool() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("connections.json");
    std::fs::write(&path, b"{ not a store")?;
    let client = AiClient::new(
        AppId::new("wordcraft")?,
        SharedStore::new(&path),
        Arc::new(MemoryStore::new()),
    );
    let tools = AppTools::with_client(client);
    match tools.status("rewrite") {
        RoleStatus::Unavailable { reason } => {
            assert!(!reason.contains("not part of the suite"), "{reason}")
        }
        RoleStatus::Ready(_) => panic!("a tool was ready with an unreadable store"),
    }
    let request = Request::new(CapabilityId::parse("text.complete")?);
    let error = tools.plan("rewrite", request).unwrap_err();
    assert!(!error.contains("not part of the suite"), "{error}");
    Ok(())
}
