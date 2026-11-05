//! A known model the adapter would refuse is reported before consent, and nothing is sent or read.
//! Saved facts, what this build can run, and what has really worked stay separate.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use craft_ai_client::{AiClient, ProviderFactory, RoleStatus, RouteSupport};
use craft_ai_core::model::InputRole;
use craft_ai_core::{
    AiError, AppId, JobId, JobSnapshot, ModelId, ModelInfo, Payload, Provider, Request, Role,
};
use craft_ai_native::secrets::{MemoryStore, SecretStore};
use craft_ai_native::shared::SharedStore;
use serde_json::json;

fn role() -> Role {
    Role {
        id: "fill".parse().unwrap(),
        label: "Fill".to_owned(),
        capability: "image.generate".parse().unwrap(),
        inputs: vec![InputRole::Prompt],
    }
}

fn request() -> Request {
    let mut request = Request::new("image.generate".parse().unwrap());
    request.inputs.push(craft_ai_core::request::Input {
        role: InputRole::Prompt,
        payload: Payload::Text {
            text: "a boat".to_owned(),
        },
    });
    request
}

/// A store whose choice is `model` on a saved connection declaring that it does the job.
fn saved(dir: &tempfile::TempDir, provider: &str, model: &str) -> SharedStore {
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let file = json!({
        "schema_version": 1,
        "connections": [{
            "id": "one",
            "label": "One",
            "provider": provider,
            "endpoint": "https://one.example.com/v1",
            "locality": "remote",
            "auth": {"kind": "api_key", "secret": "one-key"},
            "billing": "metered",
            "models": [{
                "id": model,
                "label": model,
                "capabilities": [{"capability": "image.generate", "accepts": {"prompt": 1, "mask": 1, "source": 1}}]
            }]
        }],
        "defaults": [{"role": "fill", "connection": "one", "model": model}]
    });
    std::fs::write(shared.path(), file.to_string()).unwrap();
    shared
}

/// A secret store that counts reads, so a test can prove no key was touched.
struct Counting(MemoryStore, Arc<AtomicUsize>);

impl SecretStore for Counting {
    fn name(&self) -> &'static str {
        "counting"
    }
    fn get(
        &self,
        key: &craft_ai_core::SecretRef,
    ) -> Result<Option<craft_ai_core::SecretString>, AiError> {
        self.1.fetch_add(1, Ordering::SeqCst);
        self.0.get(key)
    }
    fn set(
        &self,
        key: &craft_ai_core::SecretRef,
        value: &craft_ai_core::SecretString,
    ) -> Result<(), AiError> {
        self.0.set(key, value)
    }
    fn delete(&self, key: &craft_ai_core::SecretRef) -> Result<(), AiError> {
        self.0.delete(key)
    }
}

fn client(shared: SharedStore, factory: ProviderFactory) -> AiClient {
    let secrets = MemoryStore::new();
    secrets
        .set(
            &"one-key".parse().unwrap(),
            &craft_ai_core::SecretString::new("k"),
        )
        .unwrap();
    AiClient::new(AppId::new("photocraft").unwrap(), shared, Arc::new(secrets))
        .with_factory(factory)
}

/// A provider that must never be asked anything.
struct Untouched;

impl Provider for Untouched {
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        panic!("asking about support must not contact the service");
    }
    fn submit(&self, _: &ModelId, _: &Request) -> Result<JobId, AiError> {
        panic!("nothing may be submitted");
    }
    fn poll(&self, _: &JobId) -> Result<JobSnapshot, AiError> {
        panic!("nothing may be polled");
    }
    fn cancel(&self, _: &JobId) -> Result<(), AiError> {
        Ok(())
    }
}

#[test]
fn a_saved_omni_model_the_adapter_does_not_serve_is_not_ready() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "artcraft-omni", "not_a_real_model");
    let c = client(shared, ProviderFactory::standard());
    assert!(matches!(c.status(&role()), RoleStatus::Unavailable { .. }));
    let facts = c.route_facts(&role());
    assert_eq!(facts.configured, Ok(()), "the saved facts still resolve");
    assert!(matches!(facts.executable, RouteSupport::Unsupported { .. }));
    assert_eq!(facts.last_success_ms, None);
}

#[test]
fn it_is_refused_at_plan_before_consent_and_stays_chosen() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "artcraft-omni", "not_a_real_model");
    let before = std::fs::read(shared.path()).unwrap();
    let c = client(shared.clone(), ProviderFactory::standard());
    let refused = c.plan(&role(), request()).unwrap_err();
    assert_eq!(refused.kind, craft_ai_core::ErrorKind::Unsupported);
    assert_eq!(
        std::fs::read(shared.path()).unwrap(),
        before,
        "no other model was chosen"
    );
}

#[test]
fn a_status_check_reads_no_key_and_contacts_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "custom-refuses", "any");
    let mut factory = ProviderFactory::standard();
    factory.register("custom-refuses", |_, _| Ok(Arc::new(Untouched)));
    factory.declare_support("custom-refuses", |_| RouteSupport::Unsupported {
        reason: "Not this one.".to_owned(),
    });
    let reads = Arc::new(AtomicUsize::new(0));
    let secrets = Counting(MemoryStore::new(), reads.clone());
    let c = AiClient::new(AppId::new("photocraft").unwrap(), shared, Arc::new(secrets))
        .with_factory(factory);
    assert_eq!(
        c.status(&role()),
        RoleStatus::Unavailable {
            reason: "Not this one.".to_owned()
        }
    );
    assert_eq!(reads.load(Ordering::SeqCst), 0, "no key was read");
}

#[test]
fn an_adapter_that_does_not_say_is_untested_and_still_runs() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "custom-quiet", "any");
    let mut factory = ProviderFactory::standard();
    factory.register("custom-quiet", |_, _| Ok(Arc::new(Untouched)));
    let c = client(shared, factory);
    assert!(matches!(c.status(&role()), RoleStatus::Ready(_)));
    assert_eq!(c.route_facts(&role()).executable, RouteSupport::Untested);
    assert!(c.plan(&role(), request()).is_ok());
}

#[test]
fn a_custom_adapter_replacing_a_built_in_name_is_not_judged_by_its_list() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "artcraft-omni", "my_own_model");
    let mut factory = ProviderFactory::standard();
    factory.register("artcraft-omni", |_, _| Ok(Arc::new(Untouched)));
    let c = client(shared, factory);
    assert_eq!(c.route_facts(&role()).executable, RouteSupport::Untested);
    assert!(matches!(c.status(&role()), RoleStatus::Ready(_)));
}

#[test]
fn a_custom_adapter_can_say_it_supports_a_model() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "custom-sure", "any");
    let mut factory = ProviderFactory::standard();
    factory.register("custom-sure", |_, _| Ok(Arc::new(Untouched)));
    factory.declare_support("custom-sure", |_| RouteSupport::Supported);
    let c = client(shared, factory);
    assert_eq!(c.route_facts(&role()).executable, RouteSupport::Supported);
}

#[test]
fn a_model_known_to_be_refused_is_not_offered_or_bound() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "artcraft-omni", "not_a_real_model");
    let c = client(shared.clone(), ProviderFactory::standard());
    assert!(c.choices(&role()).unwrap().is_empty());
    assert!(c.picks(&role()).unwrap().is_empty());
    let before = std::fs::read(shared.path()).unwrap();
    let refused = c.bind(
        &role(),
        &"one".parse().unwrap(),
        &ModelId::new("not_a_real_model").unwrap(),
    );
    assert!(refused.is_err());
    assert_eq!(std::fs::read(shared.path()).unwrap(), before);
}

#[test]
fn an_unconfigured_role_says_so_and_claims_nothing_about_running() {
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let c = client(shared, ProviderFactory::standard());
    let facts = c.route_facts(&role());
    assert!(facts.configured.is_err());
    assert_eq!(facts.executable, RouteSupport::Untested);
}

#[test]
fn inputs_a_listed_model_does_not_take_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "artcraft-omni", "nano_banana");
    let c = client(shared, ProviderFactory::standard());
    assert_eq!(c.route_facts(&role()).executable, RouteSupport::Supported);
    let mut with_mask = role();
    with_mask.inputs.push(InputRole::Mask);
    assert!(matches!(
        c.route_facts(&with_mask).executable,
        RouteSupport::Unsupported { .. }
    ));
}

#[test]
fn vertex_takes_only_a_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "vertex-express", "gemini-2.5-flash-image");
    let c = client(shared, ProviderFactory::standard());
    assert_eq!(c.route_facts(&role()).executable, RouteSupport::Supported);
    let mut with_image = role();
    with_image.inputs.push(InputRole::Source);
    assert!(matches!(
        c.route_facts(&with_image).executable,
        RouteSupport::Unsupported { .. }
    ));
}
