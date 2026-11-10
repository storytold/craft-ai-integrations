//! Run records: the evidence a real run keeps. Built here from runs through the whole `AiClient`
//! path (plan, consent, start, wait), against a scripted localhost server and the mock provider,
//! so no test reaches the internet.

#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use craft_ai_client::{
    AiClient, Finished, Plan, ProviderFactory, RunOutcome, RunOutput, RunRecord,
};
use craft_ai_core::job::{Event, JobState, Output, Usage};
use craft_ai_core::mock::{MockProvider, Outcome, Script};
use craft_ai_core::{
    AiError, AppId, Binding, CapabilityId, Connection, ConsentLedger, ErrorKind, InputRole,
    ModelId, Payload, Provider, Request, Role, RoleId, SecretRef, SecretString, StoreFile,
};
use craft_ai_native::secrets::{MemoryStore, SecretStore};
use craft_ai_native::shared::SharedStore;
use serde_json::{Value, json};
use support::{Reply, Server};

const KEY: &str = "sk-live-record-needle-31";
const PROMPT: &str = "a private sentence the record must not keep";

/// The tool every test runs: a text rewrite.
fn role() -> Role {
    Role {
        id: RoleId::new("rewrite").unwrap(),
        label: "Rewrite".into(),
        capability: CapabilityId::parse("text.complete").unwrap(),
        inputs: vec![InputRole::Prompt],
    }
}

/// A rewrite request carrying `PROMPT` and one parameter, so the record has a name to list.
fn request() -> Request {
    Request::new(role().capability)
        .with_input(InputRole::Prompt, Payload::text(PROMPT))
        .with_param("max_tokens", json!(8))
}

/// A saved connection offering `gpt-4o`; a remote one uses a key named `<id>-key` and is metered.
fn connection(id: &str, provider: &str, endpoint: &str, remote: bool) -> Connection {
    let models: Vec<_> = craft_ai_openai::catalog::classify("gpt-4o")
        .into_iter()
        .collect();
    serde_json::from_value(json!({
        "id": id, "label": format!("Connection {id}"), "provider": provider,
        "endpoint": endpoint,
        "locality": if remote { "remote" } else { "local" },
        "auth": if remote { json!({"kind":"api_key","secret": format!("{id}-key")}) } else { json!({"kind":"none"}) },
        "billing": if remote { "metered" } else { "free" },
        "models": models,
    }))
    .unwrap()
}

struct World {
    _dir: tempfile::TempDir,
    client: AiClient,
}

impl World {
    /// A temporary shared store with `connection` bound to the rewrite tool, its key saved, and
    /// a client for `wordcraft` that uses `factory` when given.
    fn new(connection: Connection, factory: Option<ProviderFactory>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let shared = SharedStore::new(dir.path().join("connections.json"));
        let binding = Binding {
            role: role().id,
            connection: connection.id.clone(),
            model: ModelId::new("gpt-4o").unwrap(),
        };
        let secret = SecretRef::new(format!("{}-key", connection.id.as_str())).unwrap();
        shared
            .replace(&StoreFile {
                connections: vec![connection],
                defaults: vec![binding],
                ..StoreFile::default()
            })
            .unwrap();
        let secrets = Arc::new(MemoryStore::new());
        secrets.set(&secret, &SecretString::new(KEY)).unwrap();
        let mut client = AiClient::new(AppId::new("wordcraft").unwrap(), shared, secrets);
        if let Some(f) = factory {
            client = client.with_factory(f);
        }
        Self { _dir: dir, client }
    }

    /// Plans, agrees to exactly this plan, starts and waits: the path a recorded run takes.
    fn run(&self) -> (Plan, Finished) {
        let plan = self.client.plan(&role(), request()).unwrap();
        let mut ledger = ConsentLedger::default();
        ledger.grant(plan.disclosure());
        let job = self.client.start(&plan, &ledger).unwrap();
        let done = job
            .wait(Duration::from_millis(5), Duration::from_secs(10))
            .unwrap();
        (plan, done)
    }
}

/// A remote world whose "mock" provider follows `script`.
fn mock_world(script: Script) -> World {
    let mut f = ProviderFactory::standard();
    f.register("mock", move |conn, _secret| {
        Ok(Arc::new(MockProvider::new(conn.models.clone(), script.clone())) as Arc<dyn Provider>)
    });
    World::new(
        connection("cloud", "mock", "https://api.example.com/v1", true),
        Some(f),
    )
}

/// The record of `done`, with a fixed start time and duration so the fields can be compared.
fn record(plan: &Plan, done: &Finished) -> RunRecord {
    let started = Utc.with_ymd_and_hms(2026, 11, 10, 22, 30, 5).unwrap();
    RunRecord::new(plan, done, started, Duration::from_millis(1234))
}

/// SHA-256 of `data` as lowercase hex, worked out here rather than by the code under test.
fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A local run through the real OpenAI-compatible adapter: every field is filled from the plan
/// and the reply, and the line holds neither the prompt nor the reply.
#[test]
fn a_local_run_through_the_real_adapter_is_recorded_without_its_words() {
    let server = Server::start(|_| {
        Reply::Json(
            200,
            json!({"choices":[{"message":{"content":"pong"}}]}).to_string(),
        )
    });
    let world = World::new(connection("local", "ollama", &server.url, false), None);
    let (plan, done) = world.run();
    let rec = record(&plan, &done);

    assert_eq!(rec.outcome, RunOutcome::Succeeded);
    assert_eq!(rec.error, None);
    assert_eq!(rec.app, "wordcraft");
    assert_eq!(rec.role, "rewrite");
    assert_eq!(rec.capability, "text.complete");
    assert_eq!(rec.provider, "ollama");
    assert_eq!(rec.connection, "local");
    assert_eq!(rec.model, "gpt-4o");
    assert_eq!(rec.destination, plan.disclosure().destination);
    assert_eq!(rec.param_names, vec!["max_tokens".to_owned()]);
    assert_eq!(rec.sent, plan.disclosure().items);
    assert_eq!(rec.started_at, "2026-11-10T22:30:05.000Z");
    assert_eq!(rec.elapsed_ms, 1234);
    // The reply is measured and hashed, wherever the adapter put it.
    let text = rec
        .streamed_text
        .clone()
        .or_else(|| {
            rec.outputs.iter().find_map(|o| match o {
                RunOutput::Text { digest } => Some(digest.clone()),
                _ => None,
            })
        })
        .expect("the reply is in the record");
    assert_eq!(text.bytes, 4);
    assert_eq!(text.sha256, sha256_hex(b"pong"));

    let line = rec.to_json_line(&[KEY]).unwrap();
    assert!(!line.contains('\n'));
    assert!(!line.contains(PROMPT), "{line}");
    assert!(!line.contains("pong"), "{line}");
    let back: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(back["version"], 1);
    assert_eq!(back["outcome"], "succeeded");
    assert_eq!(back["locality"], "local");
}

/// Streamed text, an inline picture and a returned link: sizes and hashes are kept, the link
/// address and its signed token are not.
#[test]
fn results_keep_their_type_size_and_hash_but_never_a_link() {
    let picture = vec![7u8; 300];
    let link = "https://cdn.example.com/out.png?signature=secret-token";
    let world = mock_world(Script {
        running_polls: 1,
        text_chunks: vec!["po".into(), "ng".into()],
        outcome: Outcome::Succeed(vec![
            Output {
                payload: Payload::inline("image/png", picture.clone()).unwrap(),
                label: None,
            },
            Output {
                payload: Payload::handle(link, "video/mp4", Some(9000)).unwrap(),
                label: None,
            },
        ]),
    });
    let (plan, done) = world.run();
    let rec = record(&plan, &done);

    assert_eq!(rec.outcome, RunOutcome::Succeeded);
    assert_eq!(rec.provider, "mock");
    assert_eq!(rec.destination, "https://api.example.com");
    let streamed = rec.streamed_text.clone().unwrap();
    assert_eq!((streamed.bytes, streamed.sha256), (4, sha256_hex(b"pong")));
    assert_eq!(rec.outputs.len(), 2);
    match &rec.outputs[0] {
        RunOutput::Inline { media_type, digest } => {
            assert_eq!(media_type, "image/png");
            assert_eq!(digest.bytes, 300);
            assert_eq!(digest.sha256, sha256_hex(&picture));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        rec.outputs[1],
        RunOutput::Link {
            media_type: "video/mp4".into(),
            size_bytes: Some(9000),
        }
    );
    let line = rec.to_json_line(&[KEY]).unwrap();
    assert!(!line.contains("cdn.example.com"), "{line}");
    assert!(!line.contains("secret-token"), "{line}");
    let back: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(back["outputs"][0]["kind"], "inline");
    assert_eq!(back["outputs"][0]["bytes"], 300);
    assert_eq!(back["outputs"][1]["kind"], "link");
}

/// A failure keeps its kind and wording, a line that would carry the key is refused without
/// repeating it, and an empty secret does not refuse every line.
#[test]
fn a_failed_run_keeps_the_reason_and_a_key_in_it_stops_the_line() {
    let world = mock_world(Script {
        running_polls: 0,
        text_chunks: Vec::new(),
        outcome: Outcome::Fail(AiError::new(
            ErrorKind::Quota,
            format!("out of credit for {KEY}"),
        )),
    });
    let (plan, done) = world.run();
    let rec = record(&plan, &done);

    assert_eq!(rec.outcome, RunOutcome::Failed);
    let error = rec.error.clone().unwrap();
    assert_eq!(error.kind, ErrorKind::Quota);
    assert!(error.message.starts_with("out of credit"));

    let refused = rec.to_json_line(&[KEY]).unwrap_err();
    assert_eq!(refused.kind, ErrorKind::InvalidRequest);
    assert!(!refused.message.contains(KEY));
    // An empty secret would match every line; it is ignored rather than refusing everything.
    let clean = RunRecord { error: None, ..rec };
    let line = clean.to_json_line(&["", KEY]).unwrap();
    assert!(line.contains("\"outcome\":\"failed\""), "{line}");
}

/// A result built by the caller from a job that has not ended is recorded as unfinished, with its
/// notices and usage; a cancelled job reads as cancelled.
#[test]
fn notices_usage_and_a_job_not_yet_ended_are_all_kept() {
    let world = mock_world(Script::default());
    let plan = world.client.plan(&role(), request()).unwrap();
    let usage = Usage {
        input_units: Some(12),
        output_units: Some(3),
        cost_micros: Some(150),
        currency: Some("USD".into()),
    };
    let early = Finished {
        state: JobState::Running { percent: Some(40) },
        events: vec![
            Event::Notice {
                message: "kept the original link".into(),
            },
            Event::ToolCall {
                name: "lookup".into(),
                arguments: "{}".into(),
            },
        ],
        usage: Some(usage.clone()),
    };
    let rec = record(&plan, &early);
    assert_eq!(rec.outcome, RunOutcome::Unfinished);
    assert_eq!(rec.notices, vec!["kept the original link".to_owned()]);
    assert_eq!(rec.usage, Some(usage));
    assert!(rec.outputs.is_empty());
    assert_eq!(rec.streamed_text, None);

    let cancelled = Finished {
        state: JobState::Cancelled,
        events: Vec::new(),
        usage: None,
    };
    assert_eq!(record(&plan, &cancelled).outcome, RunOutcome::Cancelled);
}
