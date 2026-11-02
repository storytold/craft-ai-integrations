//! The consent prompt shows the size of a file on this machine as measured, not as claimed.
use std::sync::Arc;

use craft_ai_client::{AiClient, ProviderFactory};
use craft_ai_core::*;
use craft_ai_native::{secrets::MemoryStore, shared::SharedStore};

/// An image role whose one input is the picture to send.
fn role() -> Role {
    Role {
        id: RoleId::new("size-describe").unwrap(),
        label: "Describe".into(),
        capability: CapabilityId::parse("image.describe").unwrap(),
        inputs: vec![InputRole::Source],
    }
}

fn request(payload: Payload) -> Request {
    Request::new(role().capability).with_input(InputRole::Source, payload)
}

struct World {
    dir: tempfile::TempDir,
    client: AiClient,
}

impl World {
    /// A remote connection bound to the role, with an adapter that sends nothing.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let shared = SharedStore::new(dir.path().join("connections.json"));
        let model = ModelInfo::new(ModelId::new("size-model").unwrap(), "Size model").with(
            CapabilitySpec::new(role().capability)
                .accepting(InputRole::Source, 1)
                .requiring(InputRole::Source),
        );
        let connection: Connection = serde_json::from_value(serde_json::json!({
            "id":"size-connection", "label":"Size connection", "provider":"size-fake",
            "endpoint":"https://size.example.invalid/v1", "locality":"remote",
            "auth":{"kind":"none"}, "billing":"free", "models":[model]
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
        let mut factory = ProviderFactory::standard();
        factory.register("size-fake", |_, _| {
            Ok(Arc::new(mock::MockProvider::new(
                Vec::new(),
                mock::Script::default(),
            )))
        });
        let client = AiClient::new(
            AppId::new("size-app").unwrap(),
            shared,
            Arc::new(MemoryStore::new()),
        )
        .with_factory(factory);
        Self { dir, client }
    }

    /// The size the consent prompt would show for the one input.
    fn disclosed(&self, payload: Payload) -> Option<u64> {
        let plan = self.client.plan(&role(), request(payload)).unwrap();
        plan.disclosure().items[0].bytes
    }

    /// A `file:` address for a real file of `len` bytes in the fixture folder.
    fn file(&self, name: &str, len: usize) -> String {
        let path = self.dir.path().join(name);
        std::fs::write(&path, vec![0u8; len]).unwrap();
        url::Url::from_file_path(&path).unwrap().to_string()
    }

    fn missing(&self) -> String {
        url::Url::from_file_path(self.dir.path().join("gone.png"))
            .unwrap()
            .to_string()
    }
}

/// An app that understates (or forgets) the size of a local file cannot make the prompt show it.
#[test]
fn a_local_files_size_is_measured_not_taken_from_the_app() {
    let w = World::new();
    let uri = w.file("big.png", 4096);
    for claimed in [Some(5), None, Some(1_000_000)] {
        let payload = Payload::handle(&uri, "image/png", claimed).unwrap();
        assert_eq!(w.disclosed(payload), Some(4096), "claimed {claimed:?}");
    }
}

/// A size that could not be checked is not shown as if it were known.
#[test]
fn a_local_file_that_cannot_be_read_shows_no_size() {
    let w = World::new();
    let gone = Payload::handle(&w.missing(), "image/png", Some(5)).unwrap();
    assert_eq!(w.disclosed(gone), None);
    let folder = url::Url::from_directory_path(w.dir.path()).unwrap();
    let folder = Payload::handle(folder.as_str(), "image/png", Some(5)).unwrap();
    assert_eq!(w.disclosed(folder), None);
}

/// Handles that are not files on this machine, and inline data, keep the size they carry.
#[test]
fn other_payloads_keep_their_size() {
    let w = World::new();
    let remote =
        Payload::handle("https://cdn.example.invalid/a.png", "image/png", Some(7)).unwrap();
    assert_eq!(w.disclosed(remote), Some(7));
    let inline = Payload::inline("image/png", vec![1, 2, 3]).unwrap();
    assert_eq!(w.disclosed(inline), Some(3));
}

/// An invalid file address cannot lend credibility to an app's claimed size.
#[test]
fn malformed_file_addresses_show_no_claimed_size() {
    let w = World::new();
    for uri in ["file://[bad", "FiLe://[bad", "file://host:bad/a.png"] {
        let payload = Payload::handle(uri, "image/png", Some(5)).unwrap();
        assert_eq!(w.disclosed(payload), None, "{uri}");
    }
}
