#![allow(dead_code)]
//! A local ComfyUI connection bound to one library workflow, served by a script, shared by the
//! workflow-step tests and the plain-language test.

#[path = "../../../ai-openai/tests/support/mod.rs"]
pub mod support;

use std::sync::Arc;

use craft_ai_client::{AiClient, ProviderFactory};
use craft_ai_core::*;
use craft_ai_native::secrets::{MemoryStore, SecretStore};
use craft_ai_native::shared::SharedStore;
use serde_json::json;
use support::{Reply, Server};

const OBJECT_INFO: &str = r#"{"CLIPTextEncode":{"input":{"required":{"text":["STRING",{}]}}},
  "SaveImage":{"input":{"required":{"images":["IMAGE",{}]}}},
  "HttpPost":{"input":{"required":{}}},
  "WebhookSend":{"input":{"required":{}}}}"#;

/// The picture role the workflow is bound to.
pub fn role() -> Role {
    Role {
        id: RoleId::new("kite-picture").unwrap(),
        label: "Kite picture".into(),
        capability: CapabilityId::parse("image.generate").unwrap(),
        inputs: vec![InputRole::Prompt],
    }
}

/// A prompt-only request for the role.
pub fn request() -> Request {
    Request::new(role().capability).with_input(InputRole::Prompt, Payload::text("a kite"))
}

pub struct World {
    pub dir: tempfile::TempDir,
    pub server: Server,
    pub client: AiClient,
}

impl World {
    /// A local ComfyUI connection bound to workflow `kite` for the role, served by a script
    /// that answers the checks and queues any prompt.
    pub fn new() -> Self {
        Self::on("comfyui")
    }

    /// The same world, with the connection's kind set to `provider`. A `kite-fake` kind is
    /// registered that refuses to build, for plans that must never reach a server.
    pub fn on(provider: &str) -> Self {
        Self::with(provider, None, |_| {})
    }

    /// A world whose connection is `provider`. With `secrets`, the connection uses a key read
    /// from that store; `adjust` may change the factory before the client gets it.
    pub fn with(
        provider: &str,
        secrets: Option<Arc<dyn SecretStore>>,
        adjust: impl FnOnce(&mut ProviderFactory),
    ) -> Self {
        let auth = if secrets.is_some() {
            json!({"kind": "api_key", "secret": "comfy-key"})
        } else {
            json!({"kind": "none"})
        };
        let secrets = secrets.unwrap_or_else(|| Arc::new(MemoryStore::new()));
        let dir = tempfile::tempdir().unwrap();
        let server = Server::start(|r| match r.path.as_str() {
            "/system_stats" => Reply::Json(200, "{}".into()),
            "/object_info" => Reply::Json(200, OBJECT_INFO.into()),
            "/prompt" => Reply::Json(200, r#"{"prompt_id":"job-1"}"#.into()),
            _ => Reply::Json(404, "{}".into()),
        });
        let shared = SharedStore::new(dir.path().join("connections.json"));
        let model = ModelInfo::new(ModelId::new("kite").unwrap(), "Kite").with(
            CapabilitySpec::new(role().capability)
                .accepting(InputRole::Prompt, 1)
                .requiring(InputRole::Prompt),
        );
        let connection: Connection = serde_json::from_value(json!({
            "id": "comfyui-local", "label": "ComfyUI", "provider": provider,
            "endpoint": server.url.trim_end_matches("/v1"), "locality": "local",
            "auth": auth, "billing": "free", "models": [model]
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
        let workflows = dir.path().join("workflows");
        std::fs::create_dir_all(&workflows).unwrap();
        let client = AiClient::new(AppId::new("photocraft").unwrap(), shared, secrets)
            .with_factory({
                let mut factory = ProviderFactory::with_comfyui_library(&workflows);
                factory.register("kite-fake", |_, _| Err(AiError::unsupported("not built")));
                adjust(&mut factory);
                factory
            });
        let world = Self {
            dir,
            server,
            client,
        };
        world.write(&[]);
        world
    }

    /// Writes workflow `kite`: a prompt step and a save step, plus one node per `extra` class
    /// fed the prompt step's output.
    pub fn write(&self, extra: &[&str]) {
        let mut graph = json!({
            "1": {"class_type": "CLIPTextEncode", "inputs": {"text": "template"}},
            "2": {"class_type": "SaveImage", "inputs": {"images": ["1", 0]}}
        });
        for (i, class) in extra.iter().enumerate() {
            graph[format!("{}", 10 + i)] =
                json!({"class_type": class, "inputs": {"data": ["1", 0]}});
        }
        std::fs::write(
            self.dir.path().join("workflows/kite.json"),
            json!({
                "id": "kite", "capability": "image.generate", "graph": graph,
                "prompt": {"node": "1", "class_type": "CLIPTextEncode", "input": "text"},
                "output_node": "2"
            })
            .to_string(),
        )
        .unwrap();
    }

    /// Whether the workflow was queued on the server.
    pub fn queued(&self) -> bool {
        self.server.recorded().iter().any(|r| r.path == "/prompt")
    }
}
