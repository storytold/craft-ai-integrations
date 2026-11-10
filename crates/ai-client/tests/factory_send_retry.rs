//! The send rule set on a factory (or through `AiClient::with_retries`) reaches the worker-backed
//! adapters it builds itself, and leaves a builder an app registered alone.
#![allow(dead_code)]
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use craft_ai_client::ProviderFactory;
use craft_ai_core::conformance::run_lifecycle_paced;
use craft_ai_core::mock::{MockProvider, Script};
use craft_ai_core::{
    Backoff, CapabilityId, Connection, InputRole, JobState, ModelId, Payload, Provider, Request,
};
use serde_json::json;
use support::{Reply, Server};

/// Retries with no pause.
const QUICK: Backoff = Backoff {
    max_attempts: 3,
    first_delay: Duration::ZERO,
    budget: Duration::from_secs(5),
};

/// A local OpenAI-compatible connection to `url`.
fn connection(url: &str) -> Connection {
    let models: Vec<_> = craft_ai_openai::catalog::classify("llama3")
        .into_iter()
        .collect();
    serde_json::from_value(json!({
        "id": "here", "label": "Here", "provider": "openai", "endpoint": url,
        "locality": "local", "auth": {"kind": "none"}, "billing": "free", "models": models,
    }))
    .unwrap()
}

/// A server that refuses the first request with a 429 and answers the rest, counting requests.
fn refuses_once() -> (Server, Arc<AtomicUsize>) {
    let seen = Arc::new(AtomicUsize::new(0));
    let counter = seen.clone();
    let server = Server::start(move |_| {
        if counter.fetch_add(1, Ordering::SeqCst) == 0 {
            Reply::Json(
                429,
                json!({"error":{"message":"slow down","code":"rate_limit_exceeded"}}).to_string(),
            )
        } else {
            Reply::Json(
                200,
                json!({"choices":[{"message":{"content":"ok"}}]}).to_string(),
            )
        }
    });
    (server, seen)
}

/// Runs one prompt through `provider` to its end and returns the final state.
fn run(provider: &dyn Provider) -> JobState {
    let request = Request::new(CapabilityId::parse("text.complete").unwrap())
        .with_input(InputRole::Prompt, Payload::text("hi"));
    let pause = || std::thread::sleep(Duration::from_millis(5));
    run_lifecycle_paced(provider, &ModelId::new("llama3").unwrap(), &request, &pause)
        .unwrap()
        .state
}

/// The factory's own OpenAI builder takes the rule: a refused send is sent again.
#[test]
fn the_rule_reaches_the_factorys_own_builders() {
    let (server, seen) = refuses_once();
    let mut factory = ProviderFactory::standard();
    factory.set_send_retry(QUICK);
    let provider = factory.build(&connection(&server.url), None).unwrap();
    assert_eq!(run(&*provider), JobState::Succeeded);
    assert_eq!(seen.load(Ordering::SeqCst), 2);
}

/// NEVER turns the worker's retry off: the refusal ends the job after one request.
#[test]
fn never_turns_the_worker_retry_off() {
    let (server, seen) = refuses_once();
    let mut factory = ProviderFactory::standard();
    factory.set_send_retry(Backoff::NEVER);
    let provider = factory.build(&connection(&server.url), None).unwrap();
    assert!(matches!(run(&*provider), JobState::Failed { .. }));
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}

/// A builder an app registered for a standard name is kept when the rule changes.
#[test]
fn an_apps_own_builder_is_kept() {
    let builds = Arc::new(AtomicUsize::new(0));
    let counter = builds.clone();
    let mut factory = ProviderFactory::standard();
    factory.register("openai", move |conn, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(
            Arc::new(MockProvider::new(conn.models.clone(), Script::default()))
                as Arc<dyn Provider>,
        )
    });
    factory.set_send_retry(Backoff::NEVER);
    factory
        .build(&connection("http://127.0.0.1:9/v1"), None)
        .unwrap();
    assert_eq!(builds.load(Ordering::SeqCst), 1);
}

/// A client over one local connection to `url`, built by `make` from the shared store.
fn client_for(
    url: &str,
    dir: &std::path::Path,
    make: impl FnOnce(craft_ai_client::AiClient) -> craft_ai_client::AiClient,
) -> u64 {
    use craft_ai_client::AiClient;
    use craft_ai_core::{AppId, Binding, ConnectionId, ConsentLedger, Role, RoleId, StoreFile};
    use craft_ai_native::secrets::MemoryStore;
    use craft_ai_native::shared::SharedStore;

    let role = Role {
        id: RoleId::new("rewrite").unwrap(),
        label: "Rewrite".into(),
        capability: CapabilityId::parse("text.complete").unwrap(),
        inputs: vec![InputRole::Prompt],
    };
    let shared = SharedStore::new(dir.join("connections.json"));
    shared
        .replace(&StoreFile {
            connections: vec![connection(url)],
            defaults: vec![Binding {
                role: role.id.clone(),
                connection: ConnectionId::new("here").unwrap(),
                model: ModelId::new("llama3").unwrap(),
            }],
            ..StoreFile::default()
        })
        .unwrap();
    let client = make(AiClient::new(
        AppId::new("wordcraft").unwrap(),
        shared,
        Arc::new(MemoryStore::default()),
    ));
    let request =
        Request::new(role.capability.clone()).with_input(InputRole::Prompt, Payload::text("hi"));
    let plan = client.plan(&role, request).unwrap();
    let job = client.start(&plan, &ConsentLedger::default()).unwrap();
    let done = job
        .wait(Duration::from_millis(5), Duration::from_secs(5))
        .unwrap();
    u64::from(matches!(done.state, JobState::Failed { .. }))
}

/// `AiClient::with_retries` passes its send rule to the factory: NEVER on the client turns the
/// worker's retry off too.
#[test]
fn the_clients_send_rule_reaches_the_worker() {
    let (server, seen) = refuses_once();
    let dir = tempfile::tempdir().unwrap();
    let failed = client_for(&server.url, dir.path(), |c| {
        c.with_retries(Backoff::NEVER, QUICK)
    });
    assert_eq!(failed, 1);
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}

/// A factory set after `with_retries` takes the chosen send rule too: the order does not matter.
#[test]
fn a_later_factory_takes_the_send_rule() {
    let (server, seen) = refuses_once();
    let dir = tempfile::tempdir().unwrap();
    let failed = client_for(&server.url, dir.path(), |c| {
        c.with_retries(Backoff::NEVER, QUICK)
            .with_factory(ProviderFactory::standard())
    });
    assert_eq!(failed, 1);
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}
