//! A saved connection to a server on this computer that uses a key, over plain http. Whichever
//! program holds that port receives the key, and on a shared computer that can be another
//! account's, so the job is asked about before the key is read or anything is sent. A scripted
//! local server stands in; no real key or service is used.

#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use std::sync::Arc;
use std::time::Duration;

use craft_ai_client::{AiClient, ConnectionManager};
use craft_ai_core::{
    AppId, Binding, CapabilityId, ConnectionDraft, ConsentLedger, ErrorKind, InputRole, JobState,
    Locality, ModelId, Payload, Request, Role, RoleId,
};
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;
use craft_ai_openai::catalog::Kind;
use serde_json::json;
use support::{Reply, Server};

const KEY: &str = "local-key-51c2";

fn role() -> Role {
    Role {
        id: RoleId::new("rewrite").unwrap(),
        label: "Rewrite".into(),
        capability: CapabilityId::parse("text.complete").unwrap(),
        inputs: vec![InputRole::Prompt],
    }
}

fn prompt() -> Request {
    Request::new(role().capability).with_input(InputRole::Prompt, Payload::text("hi"))
}

/// A server that answers any chat request, keyed or not, so the test sees what it was sent.
fn server() -> Server {
    Server::start(|_| {
        Reply::Json(
            200,
            json!({"choices":[{"message":{"content":"done"}}]}).to_string(),
        )
    })
}

/// Saves a custom connection to `endpoint`, with `KEY` when `keyed`, declares one chat model
/// and binds the role to it.
fn client(dir: &tempfile::TempDir, endpoint: &str, keyed: bool) -> AiClient {
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let secrets = Arc::new(MemoryStore::new());
    let manager = ConnectionManager::new(shared.clone(), secrets.clone());
    let mut d = ConnectionDraft::custom();
    d.label = "Server here".into();
    d.endpoint = endpoint.to_owned();
    d.uses_key = keyed;
    if keyed {
        d.key = KEY.into();
    }
    d.models = craft_ai_client::declare_models(&[craft_ai_client::ModelChoice {
        id: "chat-1".into(),
        kinds: vec![Kind::Chat],
    }])
    .unwrap();
    let id = manager.save(&d).unwrap().id;
    shared
        .update(|f| {
            f.defaults.push(Binding {
                role: role().id,
                connection: id.clone(),
                model: ModelId::new("chat-1").unwrap(),
            });
        })
        .unwrap();
    AiClient::new(AppId::new("wordcraft").unwrap(), shared, secrets)
}

#[test]
fn a_key_over_plain_http_here_is_not_sent_until_the_person_agrees() {
    let (dir, s) = (tempfile::tempdir().unwrap(), server());
    let client = client(&dir, &s.url, true);
    let plan = client.plan(&role(), prompt()).unwrap();
    assert_eq!(plan.disclosure().locality, Locality::Remote);
    let e = client.start(&plan, &ConsentLedger::default()).unwrap_err();
    assert_eq!(e.kind, ErrorKind::ConsentRequired);
    assert_eq!(s.count(), 0, "nothing, and no key, reached the port");
    let mut ledger = ConsentLedger::default();
    ledger.grant(plan.disclosure());
    let done = client
        .start(&plan, &ledger)
        .unwrap()
        .wait(Duration::from_millis(5), Duration::from_secs(10))
        .unwrap();
    assert_eq!(done.state, JobState::Succeeded);
    assert!(
        s.recorded()
            .iter()
            .any(|r| r.header("authorization") == Some(&format!("Bearer {KEY}")))
    );
}

#[test]
fn a_server_here_with_no_key_still_runs_without_asking() {
    let (dir, s) = (tempfile::tempdir().unwrap(), server());
    let client = client(&dir, &s.url, false);
    let plan = client.plan(&role(), prompt()).unwrap();
    assert_eq!(plan.disclosure().locality, Locality::Local);
    let done = client
        .start(&plan, &ConsentLedger::default())
        .unwrap()
        .wait(Duration::from_millis(5), Duration::from_secs(10))
        .unwrap();
    assert_eq!(done.state, JobState::Succeeded);
}
