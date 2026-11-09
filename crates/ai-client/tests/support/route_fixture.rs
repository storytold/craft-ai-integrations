//! The saved store, tool, request, key store and adapter the route-support tests share. Every
//! connection's key is `one-key`, and nothing here contacts a service.

// Each route-support test file uses a different part of this module.
#![allow(dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use craft_ai_client::{AiClient, ProviderFactory};
use craft_ai_core::model::InputRole;
use craft_ai_core::{
    AiError, AppId, JobId, JobSnapshot, ModelId, ModelInfo, Payload, Provider, Request, Role,
};
use craft_ai_native::secrets::{MemoryStore, SecretStore};
use craft_ai_native::shared::SharedStore;
use serde_json::{Value, json};

/// An address every built-in adapter accepts besides its official one: a test server here.
pub const LOOPBACK: &str = "http://127.0.0.1:9/v1";

/// The `fill` tool for `capability`, taking a prompt.
pub fn role_for(capability: &str) -> Role {
    Role {
        id: "fill".parse().unwrap(),
        label: "Fill".to_owned(),
        capability: capability.parse().unwrap(),
        inputs: vec![InputRole::Prompt],
    }
}

/// The `fill` tool for image generation.
pub fn role() -> Role {
    role_for("image.generate")
}

/// A request for `capability` with one prompt.
pub fn request_for(capability: &str) -> Request {
    Request::new(capability.parse().unwrap()).with_input(InputRole::Prompt, Payload::text("a boat"))
}

/// An image generation request with one prompt.
pub fn request() -> Request {
    request_for("image.generate")
}

/// A store whose one connection uses `provider` at `endpoint` (on this computer when it is
/// [`LOOPBACK`]), with `model` and a second model `other` declared for `capability`, `model` taking
/// the inputs in `accepts` and chosen for the `fill` tool.
pub fn saved_at(
    dir: &tempfile::TempDir,
    provider: &str,
    endpoint: &str,
    model: &str,
    capability: &str,
    accepts: Value,
) -> SharedStore {
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let file = json!({
        "schema_version": 1,
        "connections": [{
            "id": "one",
            "label": "One",
            "provider": provider,
            "endpoint": endpoint,
            "locality": if endpoint == LOOPBACK { "local" } else { "remote" },
            "auth": {"kind": "api_key", "secret": "one-key"},
            "billing": "metered",
            "models": [
                {"id": model, "label": model,
                 "capabilities": [{"capability": capability, "accepts": accepts}]},
                {"id": "other", "label": "other",
                 "capabilities": [{"capability": capability, "accepts": {"prompt": 1}}]}
            ]
        }],
        "defaults": [{"role": "fill", "connection": "one", "model": model}]
    });
    std::fs::write(shared.path(), file.to_string()).unwrap();
    shared
}

/// A store whose choice is `model` for image generation at [`LOOPBACK`], declared to take a
/// prompt, a mask and a source image.
pub fn saved(dir: &tempfile::TempDir, provider: &str, model: &str) -> SharedStore {
    saved_at(
        dir,
        provider,
        LOOPBACK,
        model,
        "image.generate",
        json!({"prompt": 1, "mask": 1, "source": 1}),
    )
}

/// A secret store that counts reads, so a test can prove no key was touched.
pub struct Counting(pub MemoryStore, pub Arc<AtomicUsize>);

impl SecretStore for Counting {
    /// Names this store in messages.
    fn name(&self) -> &'static str {
        "counting"
    }
    /// Counts the read, then returns the saved key.
    fn get(
        &self,
        key: &craft_ai_core::SecretRef,
    ) -> Result<Option<craft_ai_core::SecretString>, AiError> {
        self.1.fetch_add(1, Ordering::SeqCst);
        self.0.get(key)
    }
    /// Saves the key without counting it as a read.
    fn set(
        &self,
        key: &craft_ai_core::SecretRef,
        value: &craft_ai_core::SecretString,
    ) -> Result<(), AiError> {
        self.0.set(key, value)
    }
    /// Removes the key.
    fn delete(&self, key: &craft_ai_core::SecretRef) -> Result<(), AiError> {
        self.0.delete(key)
    }
}

/// A client for photocraft on `shared` with its key saved, whose key reads are counted in `reads`.
pub fn counted_client(
    shared: SharedStore,
    factory: ProviderFactory,
    reads: Arc<AtomicUsize>,
) -> AiClient {
    let secrets = MemoryStore::new();
    secrets
        .set(
            &"one-key".parse().unwrap(),
            &craft_ai_core::SecretString::new("k"),
        )
        .unwrap();
    AiClient::new(
        AppId::new("photocraft").unwrap(),
        shared,
        Arc::new(Counting(secrets, reads)),
    )
    .with_factory(factory)
}

/// A client for photocraft on `shared` with its key saved.
pub fn client(shared: SharedStore, factory: ProviderFactory) -> AiClient {
    counted_client(shared, factory, Arc::new(AtomicUsize::new(0)))
}

/// A provider that must never be asked anything.
pub struct Untouched;

impl Provider for Untouched {
    /// Fails the test: asking about support must not contact the service.
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        panic!("asking about support must not contact the service");
    }
    /// Fails the test: nothing may be submitted.
    fn submit(&self, _: &ModelId, _: &Request) -> Result<JobId, AiError> {
        panic!("nothing may be submitted");
    }
    /// Fails the test: nothing may be polled.
    fn poll(&self, _: &JobId) -> Result<JobSnapshot, AiError> {
        panic!("nothing may be polled");
    }
    /// Accepts a cancel, which sends nothing.
    fn cancel(&self, _: &JobId) -> Result<(), AiError> {
        Ok(())
    }
}
