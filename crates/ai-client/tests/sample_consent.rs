//! A sample approval covers the disclosure actually shown, even if shared settings change.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use craft_ai_client::{AiClient, ProviderFactory, TestStart, ToolDesk};
use craft_ai_core::*;
use craft_ai_native::{ConsentFile, secrets::MemoryStore, shared::SharedStore};

/// A text role whose request requires consent on a remote connection.
fn role() -> Role {
    Role {
        id: RoleId::new("sample-text").unwrap(),
        label: "Sample text".into(),
        capability: CapabilityId::parse("text.complete").unwrap(),
        inputs: vec![InputRole::Prompt],
    }
}

/// A small text request; its size is part of the disclosure shown to the person.
fn request(text: &str) -> Request {
    Request::new(role().capability).with_input(InputRole::Prompt, Payload::text(text))
}

struct Fake(Arc<AtomicUsize>);
impl Provider for Fake {
    /// The fixture's connection already holds its model list.
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        Ok(Vec::new())
    }
    /// Counts actual submissions without a network call.
    fn submit(&self, _: &ModelId, _: &Request) -> Result<JobId, AiError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(JobId::new("sample-job").unwrap())
    }
    /// Returns an already finished job.
    fn poll(&self, _: &JobId) -> Result<JobSnapshot, AiError> {
        Ok(JobSnapshot::new(JobState::Succeeded))
    }
    /// The fixture has no running work to cancel.
    fn cancel(&self, _: &JobId) -> Result<(), AiError> {
        Ok(())
    }
}

struct World {
    _dir: tempfile::TempDir,
    shared: SharedStore,
    consent: ConsentFile,
    desk: ToolDesk,
    builds: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
}

impl World {
    /// An isolated remote connection with a counted provider and a real temporary consent file.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let shared = SharedStore::new(dir.path().join("connections.json"));
        let model = ModelInfo::new(ModelId::new("sample-text").unwrap(), "Sample text").with(
            CapabilitySpec::new(role().capability)
                .accepting(InputRole::Prompt, 1)
                .requiring(InputRole::Prompt),
        );
        let connection: Connection = serde_json::from_value(serde_json::json!({
            "id":"sample-remote", "label":"Sample remote", "provider":"sample-fake",
            "endpoint":"https://first.example.invalid/v1", "locality":"remote",
            "auth":{"kind":"none"}, "billing":"metered", "models":[model]
        }))
        .unwrap();
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
        let calls = Arc::new(AtomicUsize::new(0));
        let builds = Arc::new(AtomicUsize::new(0));
        let (seen, built) = (calls.clone(), builds.clone());
        let mut factory = ProviderFactory::standard();
        factory.register("sample-fake", move |_, _| {
            built.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(Fake(seen.clone())))
        });
        let consent = ConsentFile::new(dir.path().join("consent.json"));
        let client = AiClient::new(
            ToolDesk::app().unwrap(),
            shared.clone(),
            Arc::new(MemoryStore::new()),
        )
        .with_factory(factory);
        let desk = ToolDesk::new(client, shared.clone(), Some(consent.clone()));
        Self {
            _dir: dir,
            shared,
            consent,
            desk,
            builds,
            calls,
        }
    }

    /// Obtains the disclosure before any agreement has been given.
    fn shown(&self) -> Disclosure {
        match self
            .desk
            .start_test(&role(), request("fixture"), None)
            .unwrap()
        {
            TestStart::NeedsConsent { disclosure, .. } => disclosure,
            _ => panic!("the remote sample must ask first"),
        }
    }

    /// Neither consent nor a provider can be created for an unreviewed destination.
    fn assert_untouched(&self) {
        assert!(!self.consent.path().exists());
        assert_eq!(self.builds.load(Ordering::SeqCst), 0);
        assert_eq!(self.calls.load(Ordering::SeqCst), 0);
    }
}

/// Changing an endpoint while its prompt is open asks again and saves no agreement.
#[test]
fn changed_destination_asks_again_before_saving_or_building() {
    let w = World::new();
    let shown = w.shown();
    w.shared
        .update(|s| s.connections[0].endpoint = Some("https://second.example.invalid/v1".into()))
        .unwrap();
    match w
        .desk
        .start_test(&role(), request("fixture"), Some(&shown))
        .unwrap()
    {
        TestStart::NeedsConsent { disclosure, .. } => {
            assert_ne!(disclosure, shown);
            assert_eq!(disclosure.destination, "https://second.example.invalid");
        }
        _ => panic!("an approval for the first destination must not cover the second"),
    }
    w.assert_untouched();
}

/// An unchanged reviewed disclosure grants precisely that destination and subsequent samples reuse it.
#[test]
fn unchanged_disclosure_starts_and_remembers_the_shown_destination() {
    let w = World::new();
    let shown = w.shown();
    w.assert_untouched();
    assert!(matches!(
        w.desk
            .start_test(&role(), request("fixture"), Some(&shown))
            .unwrap(),
        TestStart::Started(_)
    ));
    assert_eq!(w.calls.load(Ordering::SeqCst), 1);
    let ledger = w.consent.load().unwrap();
    assert_eq!(ledger.records.len(), 1);
    assert_eq!(ledger.records[0].destination, shown.destination);
    assert!(matches!(
        w.desk
            .start_test(&role(), request("fixture"), None)
            .unwrap(),
        TestStart::Started(_)
    ));
    assert_eq!(w.calls.load(Ordering::SeqCst), 2);
}

/// Even the same destination needs a new prompt when the reviewed request's disclosure changes.
#[test]
fn changed_payload_asks_again_before_saving_or_building() {
    let w = World::new();
    let shown = w.shown();
    assert!(matches!(
        w.desk
            .start_test(&role(), request("a larger fixture"), Some(&shown))
            .unwrap(),
        TestStart::NeedsConsent { .. }
    ));
    w.assert_untouched();
}

/// After a stale approval, the person can review and approve the replacement disclosure.
#[test]
fn refreshed_disclosure_can_be_approved() {
    let w = World::new();
    let shown = w.shown();
    w.shared
        .update(|s| s.connections[0].endpoint = Some("https://second.example.invalid/v1".into()))
        .unwrap();
    let TestStart::NeedsConsent { disclosure, .. } = w
        .desk
        .start_test(&role(), request("fixture"), Some(&shown))
        .unwrap()
    else {
        panic!("the changed destination must ask again");
    };
    w.assert_untouched();
    assert!(matches!(
        w.desk
            .start_test(&role(), request("fixture"), Some(&disclosure))
            .unwrap(),
        TestStart::Started(_)
    ));
    let ledger = w.consent.load().unwrap();
    assert_eq!(ledger.records.len(), 1);
    assert_eq!(ledger.records[0].destination, disclosure.destination);
}

/// Every disclosure field participates in approval, including scope and parameter names.
#[test]
fn an_approval_must_match_every_field_of_the_current_disclosure() {
    let w = World::new();
    let shown = w.shown();
    let mut changed = Vec::new();
    let mut app = shown.clone();
    app.app = AppId::new("another-app").unwrap();
    changed.push(app);
    let mut connection = shown.clone();
    connection.connection = ConnectionId::new("another-connection").unwrap();
    changed.push(connection);
    let mut capability = shown.clone();
    capability.capability = CapabilityId::parse("image.generate").unwrap();
    changed.push(capability);
    let mut locality = shown.clone();
    locality.locality = Locality::Local;
    changed.push(locality);
    let mut destination = shown.clone();
    destination.destination = "https://another.example.invalid".into();
    changed.push(destination);
    let mut items = shown.clone();
    items.items.clear();
    changed.push(items);
    let mut params = shown.clone();
    params.param_names.push("quality".into());
    changed.push(params);
    for reviewed in changed {
        let TestStart::NeedsConsent { disclosure, .. } = w
            .desk
            .start_test(&role(), request("fixture"), Some(&reviewed))
            .unwrap()
        else {
            panic!("a different disclosure must ask again");
        };
        assert_eq!(disclosure, shown);
        w.assert_untouched();
    }
}
