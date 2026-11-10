//! The store, tool desk and helpers the model assignment tests share: one connection `one` whose
//! model `reported` the provider says can do images, and `bare`, which can do nothing yet.

// Each assignment test file uses a different part of this module.
#![allow(dead_code)]

use std::sync::Arc;

use craft_ai_client::{Ability, AiClient, ChoiceScope, ToolDesk};
use craft_ai_core::{AppId, ConnectionId, ModelId, Role, RoleId, StoreFile};
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;
use serde_json::json;

/// A temporary store and a tool desk over it. The folder lives as long as the world.
pub struct World {
    _dir: tempfile::TempDir,
    pub shared: SharedStore,
    pub desk: ToolDesk,
}

/// A store at a new temporary path, written from `file` when given, and a tool desk over it.
pub fn world(file: Option<serde_json::Value>) -> World {
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    if let Some(file) = file {
        std::fs::write(shared.path(), file.to_string()).unwrap();
    }
    let client = AiClient::new(
        ToolDesk::app().unwrap(),
        shared.clone(),
        Arc::new(MemoryStore::new()),
    );
    let desk = ToolDesk::new(client, shared.clone(), None);
    World {
        _dir: dir,
        shared,
        desk,
    }
}

/// One connection `one`: `reported` can do images (the provider said so), `bare` can do nothing yet.
pub fn file() -> serde_json::Value {
    json!({
        "schema_version": 1,
        "connections": [{
            "id": "one",
            "label": "One",
            "provider": "openai",
            "endpoint": "https://one.example.com/v1",
            "locality": "remote",
            "auth": {"kind": "none"},
            "billing": "metered",
            "models": [
                {
                    "id": "reported",
                    "label": "Reported",
                    "capabilities": [{"capability": "image.generate", "accepts": {"prompt": 1}}]
                },
                {"id": "bare", "label": "Bare"}
            ]
        }]
    })
}

/// A tool `id` for image generation that takes a prompt.
pub fn tool(id: &str) -> Role {
    Role {
        id: RoleId::new(id).unwrap(),
        label: id.to_owned(),
        capability: "image.generate".parse().unwrap(),
        inputs: vec![craft_ai_core::model::InputRole::Prompt],
    }
}

/// The connection in [`file`].
pub fn one() -> ConnectionId {
    ConnectionId::new("one").unwrap()
}

/// A model id.
pub fn model(id: &str) -> ModelId {
    ModelId::new(id).unwrap()
}

/// The scope of app `id`'s own choice.
pub fn app(id: &str) -> ChoiceScope {
    ChoiceScope::App(AppId::new(id).unwrap())
}

/// The person says the model can do the job.
pub fn declared() -> Ability {
    Ability::DeclaredByPerson {
        label: "Added".to_owned(),
    }
}

/// The store as saved now.
pub fn saved(w: &World) -> StoreFile {
    w.shared.load().unwrap()
}

/// The store file's exact bytes.
pub fn bytes(w: &World) -> Vec<u8> {
    std::fs::read(w.shared.path()).unwrap()
}
