//! Refusing a stale saved probe must not serialise or recreate the shared store.
#[path = "support/model_probe.rs"]
mod model_probe;

use std::sync::{Arc, Mutex};

use craft_ai_client::{ConnectionManager, ProviderFactory};
use craft_ai_core::{Connection, ConnectionId, ErrorKind, ModelId, ModelInfo, StoreFile};
use craft_ai_native::{secrets::MemoryStore, shared::SharedStore};
use model_probe::ModelProbe;

/// A valid connection with no real credential or service.
fn connection() -> Connection {
    serde_json::from_value(serde_json::json!({
        "id":"abort-probe", "label":"Abort probe", "provider":"abort-fake",
        "endpoint":"https://first.example.invalid/v1", "locality":"remote",
        "auth":{"kind":"none"}, "billing":"free", "models":[]
    }))
    .unwrap()
}

type Edit = Arc<dyn Fn(&SharedStore) + Send + Sync>;
type SavedBytes = Arc<Mutex<Option<Vec<u8>>>>;

struct World {
    _dir: tempfile::TempDir,
    shared: SharedStore,
    manager: ConnectionManager,
    after_edit: SavedBytes,
}
impl World {
    /// A temporary store and stand-in probe whose callback edits the raw on-disk file.
    fn new(edit: impl Fn(&SharedStore) + Send + Sync + 'static) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let shared = SharedStore::new(dir.path().join("connections.json"));
        shared
            .replace(&StoreFile {
                connections: vec![connection()],
                ..StoreFile::default()
            })
            .unwrap();
        let after_edit = Arc::new(Mutex::new(None));
        let (store, after, edit) = (shared.clone(), after_edit.clone(), Arc::new(edit) as Edit);
        let mut factory = ProviderFactory::standard();
        factory.register("abort-fake", move |_, _| {
            let (store, after, edit) = (store.clone(), after.clone(), edit.clone());
            Ok(Arc::new(ModelProbe::new(move || {
                (edit)(&store);
                *after.lock().unwrap() = match std::fs::read(store.path()) {
                    Ok(bytes) => Some(bytes),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => panic!("cannot read the fixture after its edit: {e}"),
                };
                Ok(vec![ModelInfo::new(
                    ModelId::new("old-answer").unwrap(),
                    "Old answer",
                )])
            })))
        });
        let manager = ConnectionManager::new(shared.clone(), Arc::new(MemoryStore::new()))
            .with_probe_factory(factory);
        Self {
            _dir: dir,
            shared,
            manager,
            after_edit,
        }
    }

    /// A refused probe leaves the exact external bytes, or absence of a file, untouched.
    fn assert_rejected_without_writing(&self) {
        let error = self
            .manager
            .test_saved(&ConnectionId::new("abort-probe").unwrap(), true)
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidRequest);
        match self.after_edit.lock().unwrap().as_ref() {
            Some(bytes) => assert!(
                std::fs::read(self.shared.path()).unwrap() == *bytes,
                "a refused probe rewrote the external file"
            ),
            None => assert!(
                !self.shared.path().exists(),
                "a refused probe recreated the deleted store"
            ),
        }
    }
}

/// Writes a same-schema file directly, retaining a nested field unknown to this build and its formatting.
fn external_edit(shared: &SharedStore, remove_tested_connection: bool) {
    let mut raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(shared.path()).unwrap()).unwrap();
    raw["connections"][0]["endpoint"] = serde_json::json!("https://second.example.invalid/v1");
    raw["connections"][0]["future_transport"] = serde_json::json!({"keep":"external-setting"});
    if remove_tested_connection {
        raw["connections"][0]["id"] = serde_json::json!("another-connection");
    }
    let bytes = format!("\n{}\n\n", serde_json::to_string(&raw).unwrap()).into_bytes();
    std::fs::write(shared.path(), bytes).unwrap();
    assert!(
        shared.load().is_ok(),
        "the external edit must be valid in the same schema"
    );
}

/// A changed endpoint must keep an external writer's nested fields and exact bytes on refusal.
#[test]
fn a_changed_connection_preserves_externally_written_nested_settings() {
    World::new(|shared| external_edit(shared, false)).assert_rejected_without_writing();
}

/// If the tested connection vanished, refusal must not rewrite unrelated connections either.
#[test]
fn a_missing_connection_preserves_externally_written_nested_settings() {
    World::new(|shared| external_edit(shared, true)).assert_rejected_without_writing();
}

/// Deleting the store during a probe must not cause an empty store to be recreated on refusal.
#[test]
fn a_deleted_store_is_not_recreated_by_a_refused_probe() {
    World::new(|shared| std::fs::remove_file(shared.path()).unwrap())
        .assert_rejected_without_writing();
}
