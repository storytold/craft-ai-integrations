//! App-scoped plans cannot be reused through another app's client.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use craft_ai_client::{AiClient, ProviderFactory};
use craft_ai_core::*;
use craft_ai_native::{HistoryFile, secrets::SecretStore, shared::SharedStore};

/// A text role available through the fixture's shared binding.
fn role() -> Role {
    Role {
        id: RoleId::new("scope-text").unwrap(),
        label: "Scope text".into(),
        capability: CapabilityId::parse("text.complete").unwrap(),
        inputs: vec![InputRole::Prompt],
    }
}

/// A small in-memory request with no real user data.
fn request() -> Request {
    Request::new(role().capability).with_input(InputRole::Prompt, Payload::text("fixture"))
}

#[derive(Default)]
struct Keys(AtomicUsize);
impl SecretStore for Keys {
    /// Identifies this stand-in store without exposing its values.
    fn name(&self) -> &'static str {
        "counted-fixture"
    }
    /// Counts key access so a refusal must happen before a secret is requested.
    fn get(&self, _: &SecretRef) -> Result<Option<SecretString>, AiError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Some(SecretString::new("fixture-only")))
    }
    /// This fixture never saves credentials.
    fn set(&self, _: &SecretRef, _: &SecretString) -> Result<(), AiError> {
        Err(AiError::unsupported("read only"))
    }
    /// This fixture never removes credentials.
    fn delete(&self, _: &SecretRef) -> Result<(), AiError> {
        Err(AiError::unsupported("read only"))
    }
}

struct Fake(Arc<AtomicUsize>);
impl Provider for Fake {
    /// The model list is supplied by the shared fixture.
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        Ok(Vec::new())
    }
    /// Counts the submission without making a network call.
    fn submit(&self, _: &ModelId, _: &Request) -> Result<JobId, AiError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(JobId::new("scope-job").unwrap())
    }
    /// Answers with a completed job.
    fn poll(&self, _: &JobId) -> Result<JobSnapshot, AiError> {
        Ok(JobSnapshot::new(JobState::Succeeded))
    }
    /// No fixture work remains to cancel.
    fn cancel(&self, _: &JobId) -> Result<(), AiError> {
        Ok(())
    }
}

struct World {
    _dir: tempfile::TempDir,
    shared: SharedStore,
    keys: Arc<Keys>,
    builds: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
    history: HistoryFile,
}
impl World {
    /// A real temporary store and history file with counted, entirely in-memory execution.
    fn new(remote: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let shared = SharedStore::new(dir.path().join("connections.json"));
        let model = ModelInfo::new(ModelId::new("scope-model").unwrap(), "Scope model").with(
            CapabilitySpec::new(role().capability)
                .accepting(InputRole::Prompt, 1)
                .requiring(InputRole::Prompt),
        );
        let connection: Connection = serde_json::from_value(serde_json::json!({
            "id":"scope-connection", "label":"Scope connection", "provider":"scope-fake",
            "endpoint": if remote {"https://scope.example.invalid/v1"} else {"http://127.0.0.1:9/v1"},
            "locality": if remote {"remote"} else {"local"},
            "auth":{"kind":"api_key", "secret":"scope-key"}, "billing":"free", "models":[model]
        })).unwrap();
        shared
            .replace(&StoreFile {
                defaults: vec![Binding {
                    role: role().id,
                    connection: connection.id.clone(),
                    model: model.id,
                }],
                connections: vec![connection],
                ..StoreFile::default()
            })
            .unwrap();
        let history = HistoryFile::new(dir.path().join("jobs.jsonl"));
        Self {
            _dir: dir,
            shared,
            history,
            keys: Arc::new(Keys::default()),
            builds: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Builds a client for an app while sharing only the fixture's connection and counted services.
    fn client(&self, app: &str) -> AiClient {
        let mut factory = ProviderFactory::standard();
        let (builds, calls) = (self.builds.clone(), self.calls.clone());
        factory.register("scope-fake", move |_, _| {
            builds.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(Fake(calls.clone())))
        });
        AiClient::new(
            AppId::new(app).unwrap(),
            self.shared.clone(),
            self.keys.clone(),
        )
        .with_factory(factory)
        .with_history(self.history.clone())
    }

    /// No execution or history side effect is allowed after an app-scope refusal.
    fn assert_untouched(&self) {
        assert_eq!(self.keys.0.load(Ordering::SeqCst), 0);
        assert_eq!(self.builds.load(Ordering::SeqCst), 0);
        assert_eq!(self.calls.load(Ordering::SeqCst), 0);
        assert!(!self.history.path().exists());
    }
}

/// Matching consent for app A cannot let app B start A's plan, including on local routes.
#[test]
fn another_apps_plan_is_refused_before_key_provider_and_history() {
    for remote in [true, false] {
        let w = World::new(remote);
        let plan = w.client("app-a").plan(&role(), request()).unwrap();
        let mut ledger = ConsentLedger::default();
        ledger.grant(plan.disclosure());
        let error = w.client("app-b").start(&plan, &ledger).unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidRequest);
        w.assert_untouched();
    }
}

/// Two clients for the same app can still share an approved plan.
#[test]
fn a_second_client_for_the_same_app_can_start_the_plan() {
    let w = World::new(true);
    let plan = w.client("app-a").plan(&role(), request()).unwrap();
    let mut ledger = ConsentLedger::default();
    ledger.grant(plan.disclosure());
    w.client("app-a").start(&plan, &ledger).unwrap();
    assert_eq!(w.calls.load(Ordering::SeqCst), 1);
    assert_eq!(w.keys.0.load(Ordering::SeqCst), 1);
    assert!(w.history.path().exists());
}

/// App A's consent still does not cover a fresh plan belonging to app B.
#[test]
fn another_apps_consent_cannot_cover_this_apps_own_plan() {
    let w = World::new(true);
    let a = w.client("app-a").plan(&role(), request()).unwrap();
    let b = w.client("app-b");
    let plan = b.plan(&role(), request()).unwrap();
    let mut ledger = ConsentLedger::default();
    ledger.grant(a.disclosure());
    assert_eq!(
        b.start(&plan, &ledger).unwrap_err().kind,
        ErrorKind::ConsentRequired
    );
    w.assert_untouched();
}

/// A foreign plan is an app-scope error even when no consent has been granted.
#[test]
fn app_scope_is_checked_before_asking_for_consent() {
    let w = World::new(true);
    let plan = w.client("app-a").plan(&role(), request()).unwrap();
    assert_eq!(
        w.client("app-b")
            .start(&plan, &ConsentLedger::default())
            .unwrap_err()
            .kind,
        ErrorKind::InvalidRequest
    );
    w.assert_untouched();
}
