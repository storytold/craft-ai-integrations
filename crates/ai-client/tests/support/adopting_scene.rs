//! The world every adoption guide block runs in, with no network and no real settings.
//!
//! The launcher has already set things up: one tool ("Tighten text"), one connection ("Tidy", either
//! on this computer or online with a saved key) and the person's choice of its model. An app then
//! builds a client that ships a scripted adapter for the "tidy" service. Guide blocks start from
//! here, so they can show only the lines a reader needs.

// Each guide test file uses a different part of this module.
#![allow(dead_code)]

use std::sync::Arc;

use craft_ai_client::{AiClient, ProviderFactory, ToolDesk};
use craft_ai_core::mock::{MockProvider, Outcome, Script};
use craft_ai_core::{
    AppId, CapabilityId, CapabilitySpec, Connection, ConnectionId, InputRole, ModelId, ModelInfo,
    Output, Payload, Provider, Request, Role, RoleId, SecretRef, SecretString,
};
use craft_ai_native::secrets::{MemoryStore, SecretStore};
use craft_ai_native::shared::SharedStore;

/// What a guide test returns, so a block can use `?` as an app would.
pub type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The launcher's side of the story, already done.
pub struct Scene {
    /// Keeps the temporary folder alive until the test ends.
    pub _dir: tempfile::TempDir,
    pub shared: SharedStore,
    pub keys: Arc<MemoryStore>,
    pub role: Role,
    pub model: ModelInfo,
    pub launcher: ToolDesk,
}

impl Scene {
    /// A temporary store with the tool declared and its model chosen. `online` makes the
    /// connection a metered remote service with a saved key, so using it needs consent.
    pub fn new(online: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let shared = SharedStore::new(dir.path().join("connections.json"));
        let keys = Arc::new(MemoryStore::new());
        let role = Role {
            id: RoleId::new("tighten-text")?,
            label: "Tighten text".into(),
            capability: CapabilityId::parse("text.complete")?,
            inputs: vec![InputRole::Prompt],
        };
        let model = ModelInfo::new(ModelId::new("tidy-1")?, "Tidy 1").with(
            CapabilitySpec::new(role.capability.clone())
                .accepting(InputRole::Prompt, 1)
                .requiring(InputRole::Prompt),
        );
        let connection = if online {
            keys.set(
                &SecretRef::new("tidy-key")?,
                &SecretString::new("not-a-real-key"),
            )?;
            serde_json::json!({
                "id": "tidy", "label": "Tidy (online)", "provider": "tidy",
                "endpoint": "https://tidy.example.com/v1", "locality": "remote",
                "auth": {"kind": "api_key", "secret": "tidy-key"},
                "billing": "metered", "models": [model.clone()],
            })
        } else {
            serde_json::json!({
                "id": "tidy", "label": "Tidy (on this computer)", "provider": "tidy",
                "locality": "local", "auth": {"kind": "none"},
                "billing": "free", "models": [model.clone()],
            })
        };
        let connection: Connection = serde_json::from_value(connection)?;
        shared.update(|store| store.connections.push(connection))?;
        let launcher = ToolDesk::new(
            AiClient::new(AppId::new("launcher")?, shared.clone(), keys.clone()),
            shared.clone(),
            None,
        );
        launcher.register_tool(role.clone())?;
        launcher.choose(&role, &ConnectionId::new("tidy")?, &model.id)?;
        Ok(Self {
            _dir: dir,
            shared,
            keys,
            role,
            model,
            launcher,
        })
    }
}

/// The app's client. This app ships an adapter for the "tidy" service (a scripted stand-in that
/// answers "Shorter."), so the launcher's choice is ready here with no setup.
pub fn scripted_client(
    shared: &SharedStore,
    keys: &Arc<MemoryStore>,
) -> Result<AiClient, Box<dyn std::error::Error>> {
    let mut factory = ProviderFactory::standard();
    factory.register("tidy", |connection, _key| {
        let script = Script {
            running_polls: 1,
            text_chunks: vec![],
            outcome: Outcome::Succeed(vec![Output {
                payload: Payload::text("Shorter."),
                label: None,
            }]),
        };
        Ok(Arc::new(MockProvider::new(connection.models.clone(), script)) as Arc<dyn Provider>)
    });
    Ok(AiClient::new(AppId::new("wordcraft")?, shared.clone(), keys.clone()).with_factory(factory))
}

/// A request for the tool: one prompt.
pub fn tighten_request(role: &Role) -> Request {
    Request::new(role.capability.clone())
        .with_input(InputRole::Prompt, Payload::text("Tighten this paragraph."))
}
