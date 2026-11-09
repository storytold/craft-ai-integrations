//! `Plan::estimate`: the price an app can show before the person agrees to send. It comes from
//! the saved model's pricing, needs no key, builds no adapter, and is what the request history
//! records when the adapter gives no figure of its own. Providers are fakes; no network is used.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use craft_ai_client::{AiClient, ProviderFactory};
use craft_ai_core::{
    AiError, AppId, Billing, Binding, CapabilityId, CapabilitySpec, Connection, ConnectionId,
    ConsentLedger, CostEstimate, InputRole, JobId, JobSnapshot, JobState, ModelId, ModelInfo,
    Payload, Pricing, Provider, Request, Role, RoleId, SecretRef, SecretString, StoreFile,
};
use craft_ai_native::HistoryFile;
use craft_ai_native::secrets::{MemoryStore, SecretStore};
use craft_ai_native::shared::SharedStore;
use serde_json::json;

/// A provider that finishes at once and reports `own` as its estimate.
struct Fake {
    own: Option<CostEstimate>,
}

impl Provider for Fake {
    /// No models: the store already holds the one in use.
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        Ok(Vec::new())
    }
    /// The adapter's own figure, which may be absent.
    fn estimate(&self, _: &ModelId, _: &Request) -> Result<Option<CostEstimate>, AiError> {
        Ok(self.own.clone())
    }
    /// Accepts every request under one fixed job id.
    fn submit(&self, _: &ModelId, _: &Request) -> Result<JobId, AiError> {
        JobId::new("img-1").map_err(|e| AiError::invalid(e.to_string()))
    }
    /// Reports the job as done on the first poll.
    fn poll(&self, _: &JobId) -> Result<JobSnapshot, AiError> {
        Ok(JobSnapshot::new(JobState::Succeeded))
    }
    /// Nothing to cancel.
    fn cancel(&self, _: &JobId) -> Result<(), AiError> {
        Ok(())
    }
}

/// The image role every test plans for.
fn role() -> Role {
    Role {
        id: RoleId::new("picture").unwrap(),
        label: "Picture".into(),
        capability: CapabilityId::parse("image.generate").unwrap(),
        inputs: vec![InputRole::Prompt],
    }
}

/// A prompt asking for `outputs` pictures.
fn request(outputs: u32) -> Request {
    let mut r =
        Request::new(role().capability).with_input(InputRole::Prompt, Payload::text("a red kite"));
    r.outputs = outputs;
    r
}

/// The saved model, priced at `pricing`, returning up to four pictures.
fn model(pricing: Option<Pricing>) -> ModelInfo {
    let mut m = ModelInfo::new(ModelId::new("painter").unwrap(), "Painter");
    m.capabilities = vec![
        CapabilitySpec::new(role().capability)
            .accepting(InputRole::Prompt, 1)
            .requiring(InputRole::Prompt)
            .max_outputs(4),
    ];
    m.pricing = pricing;
    m
}

/// A client whose store holds one metered connection and its model, and how many times the
/// adapter was built.
struct World {
    _dir: tempfile::TempDir,
    client: AiClient,
    built: Arc<AtomicUsize>,
}

/// Builds a [`World`] around `model`. The fake adapter reports `own` as its estimate, and a key
/// is stored only when `with_key` is true.
fn world(model: ModelInfo, own: Option<CostEstimate>, with_key: bool) -> World {
    let dir = tempfile::tempdir().unwrap();
    let connection: Connection = serde_json::from_value(json!({
        "id": "hosted", "label": "Hosted", "provider": "estimate-fake",
        "endpoint": "https://api.example.com", "locality": "remote",
        "auth": {"kind": "api_key", "secret": "hosted-key"}, "billing": "metered",
        "models": [model],
    }))
    .unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    shared
        .replace(&StoreFile {
            connections: vec![connection],
            defaults: vec![Binding {
                role: role().id,
                connection: ConnectionId::new("hosted").unwrap(),
                model: ModelId::new("painter").unwrap(),
            }],
            ..StoreFile::default()
        })
        .unwrap();
    let secrets = MemoryStore::new();
    if with_key {
        secrets
            .set(
                &SecretRef::new("hosted-key").unwrap(),
                &SecretString::new("sk-estimate-test"),
            )
            .unwrap();
    }
    let built = Arc::new(AtomicUsize::new(0));
    let count = built.clone();
    let mut factory = ProviderFactory::standard();
    factory.register("estimate-fake", move |_, _| {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(Fake { own: own.clone() }))
    });
    let client = AiClient::new(AppId::new("photocraft").unwrap(), shared, Arc::new(secrets))
        .with_factory(factory)
        .with_history(HistoryFile::new(dir.path().join("jobs.jsonl")));
    World {
        _dir: dir,
        client,
        built,
    }
}

/// A per-picture price of 0.039 USD.
fn priced() -> Option<Pricing> {
    Some(Pricing {
        per_image_micros: Some(39_000),
        ..Pricing::default()
    })
}

/// The plan carries the price for every picture asked for, with the connection's billing,
/// before any key exists and without building the adapter.
#[test]
fn the_plan_carries_the_price_with_no_key_and_no_adapter() {
    let w = world(model(priced()), None, false);
    let plan = w.client.plan(&role(), request(3)).unwrap();
    assert_eq!(
        plan.estimate(),
        Some(&CostEstimate {
            micros: 117_000,
            currency: "USD".into(),
            billing: Billing::Metered,
        })
    );
    assert_eq!(
        w.built.load(Ordering::SeqCst),
        0,
        "no adapter before consent"
    );
}

/// A model with no saved price gives a plan with no estimate.
#[test]
fn an_unpriced_model_gives_no_estimate() {
    let w = world(model(None), None, false);
    let plan = w.client.plan(&role(), request(1)).unwrap();
    assert_eq!(plan.estimate(), None);
}

/// Agrees to `plan`, runs it to the end, and returns the estimate history recorded.
fn recorded(w: &World, outputs: u32) -> Option<CostEstimate> {
    let plan = w.client.plan(&role(), request(outputs)).unwrap();
    let mut ledger = ConsentLedger::default();
    ledger.grant(plan.disclosure());
    let job = w.client.start(&plan, &ledger).unwrap();
    job.wait(Duration::from_millis(1), Duration::from_secs(5))
        .unwrap();
    let log = w.client.history().unwrap();
    log.entries[0].start.estimate.clone()
}

/// When the adapter gives no figure, history records the plan's.
#[test]
fn history_records_the_plan_estimate() {
    let w = world(model(priced()), None, true);
    assert_eq!(recorded(&w, 2).map(|c| c.micros), Some(78_000));
}

/// When the adapter gives its own figure, history records that one instead.
#[test]
fn the_adapter_figure_wins_in_history() {
    let own = CostEstimate {
        micros: 5,
        currency: "USD".into(),
        billing: Billing::Metered,
    };
    let w = world(model(priced()), Some(own), true);
    assert_eq!(recorded(&w, 2).map(|c| c.micros), Some(5));
}

/// A refusal before the adapter is built (here, no key) still records the plan's estimate.
#[test]
fn a_refusal_before_the_adapter_records_the_plan_estimate() {
    let w = world(model(priced()), None, false);
    let plan = w.client.plan(&role(), request(1)).unwrap();
    let mut ledger = ConsentLedger::default();
    ledger.grant(plan.disclosure());
    assert!(w.client.start(&plan, &ledger).is_err());
    let log = w.client.history().unwrap();
    let start = &log.entries[0].start;
    assert_eq!(start.estimate.as_ref().map(|c| c.micros), Some(39_000));
}
