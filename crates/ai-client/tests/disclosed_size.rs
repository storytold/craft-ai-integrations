//! The consent prompt shows the size of a file on this machine as measured, not as claimed.
use std::sync::Arc;

use craft_ai_client::{AiClient, ProviderFactory};
use craft_ai_core::*;
use craft_ai_native::{secrets::MemoryStore, shared::SharedStore};
use serde_json::json;

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

/// A handle as it arrives in a deserialized request, without `Payload::handle`'s checks.
fn raw_handle(uri: &str, size: u64) -> Payload {
    serde_json::from_value(json!({
        "type": "handle", "uri": uri, "media_type": "image/png", "size_bytes": size
    }))
    .unwrap()
}

struct World {
    dir: tempfile::TempDir,
    client: AiClient,
}

impl World {
    /// A remote connection bound to the role, with a scripted adapter that sends nothing.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let spec = CapabilitySpec::new(role().capability)
            .accepting(InputRole::Source, 1)
            .requiring(InputRole::Source);
        let model = ModelInfo::new(ModelId::new("size-model").unwrap(), "Size model").with(spec);
        let store: StoreFile = serde_json::from_value(json!({
            "schema_version": store::SCHEMA_VERSION,
            "defaults": [{"role": role().id, "connection": "size-link", "model": model.id}],
            "connections": [{
                "id": "size-link", "label": "Size link", "provider": "size-fake",
                "endpoint": "https://size.example.invalid/v1", "locality": "remote",
                "auth": {"kind": "none"}, "billing": "free", "models": [model]
            }]
        }))
        .unwrap();
        let shared = SharedStore::new(dir.path().join("connections.json"));
        shared.replace(&store).unwrap();
        let mut factory = ProviderFactory::standard();
        factory.register("size-fake", |_, _| {
            let script = mock::Script::default();
            Ok(Arc::new(mock::MockProvider::new(Vec::new(), script)))
        });
        let secrets = Arc::new(MemoryStore::new());
        let app = AppId::new("size-app").unwrap();
        let client = AiClient::new(app, shared, secrets).with_factory(factory);
        Self { dir, client }
    }

    /// The disclosure the consent prompt would show.
    fn plan(&self, payload: Payload) -> Disclosure {
        let plan = self.client.plan(&role(), request(payload)).unwrap();
        plan.disclosure().clone()
    }

    /// The size the consent prompt would show for the one input.
    fn disclosed(&self, payload: Payload) -> Option<u64> {
        self.plan(payload).items[0].bytes
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
    // The uploaders' parser ignores spaces around an address, so the same file is measured.
    let padded = Payload::handle(&format!(" {uri} "), "image/png", Some(5)).unwrap();
    assert_eq!(w.disclosed(padded), Some(4096));
}

/// A new plan for the same address measures the file again, so a changed file is a changed
/// disclosure and is asked about again.
#[test]
fn a_replanned_file_shows_its_current_size() {
    let w = World::new();
    let uri = w.file("changing.png", 5);
    let first = w.plan(Payload::handle(&uri, "image/png", Some(5)).unwrap());
    w.file("changing.png", 9);
    let second = w.plan(Payload::handle(&uri, "image/png", Some(5)).unwrap());
    assert_eq!(first.items[0].bytes, Some(5));
    assert_eq!(second.items[0].bytes, Some(9));
    assert_ne!(first, second);
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

/// An address that does not parse cannot lend credibility to an app's claimed size, however
/// its scheme is spelled or padded.
#[test]
fn addresses_that_do_not_parse_show_no_size() {
    let w = World::new();
    for uri in [
        "file://[bad",
        "FiLe://[bad",
        "file://host:bad/a.png",
        " file://[bad",
        "not an address",
    ] {
        let payload = Payload::handle(uri, "image/png", Some(5)).unwrap();
        assert_eq!(w.disclosed(payload), None, "{uri:?}");
    }
    // A deserialized request skips `Payload::handle`, so control characters can reach here.
    for uri in ["\tfile://[bad", "fi\nle://[bad", "\u{1}file://[bad"] {
        assert_eq!(w.disclosed(raw_handle(uri, 5)), None, "{uri:?}");
    }
}
