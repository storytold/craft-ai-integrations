//! A saved model probe must not overwrite changes made while it is asking the service.
#[path = "support/model_probe.rs"]
mod model_probe;

use std::sync::{Arc, Mutex};

use craft_ai_client::{ConnectionManager, ProviderFactory};
use craft_ai_core::*;
use craft_ai_native::{
    secrets::{MemoryStore, SecretStore},
    shared::SharedStore,
};
use model_probe::ModelProbe;

/// A plain model identity, sufficient for testing saved-list provenance.
fn model(id: &str) -> ModelInfo {
    ModelInfo::new(ModelId::new(id).unwrap(), id)
}

/// A valid saved connection whose model list has not yet been refreshed.
fn connection() -> Connection {
    serde_json::from_value(serde_json::json!({
        "id":"probe-connection", "label":"Probe connection", "provider":"probe-fake",
        "endpoint":"https://first.example.invalid/v1", "locality":"remote",
        "auth":{"kind":"api_key", "secret":"probe-key"}, "billing":"free", "models":[model("saved-before")]
    }))
    .unwrap()
}

type Edit = Arc<dyn Fn(&mut StoreFile) + Send + Sync>;

struct World {
    _dir: tempfile::TempDir,
    shared: SharedStore,
    manager: ConnectionManager,
    after_edit: Arc<Mutex<Option<Vec<u8>>>>,
}
impl World {
    /// A temporary shared file whose stand-in probe applies `edit` before returning its answer.
    fn new(edit: impl Fn(&mut StoreFile) + Send + Sync + 'static) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let shared = SharedStore::new(dir.path().join("connections.json"));
        shared
            .replace(&StoreFile {
                connections: vec![connection()],
                ..StoreFile::default()
            })
            .unwrap();
        let after_edit = Arc::new(Mutex::new(None));
        let mut factory = ProviderFactory::standard();
        let (store, after, edit) = (shared.clone(), after_edit.clone(), Arc::new(edit) as Edit);
        factory.register("probe-fake", move |_, _| {
            let (store, after, edit) = (store.clone(), after.clone(), edit.clone());
            Ok(Arc::new(ModelProbe::new(move || {
                store.update(|s| (edit)(s)).unwrap();
                *after.lock().unwrap() = Some(std::fs::read(store.path()).unwrap());
                Ok(vec![model("reported-old")])
            })))
        });
        let keys = Arc::new(MemoryStore::new());
        keys.set(
            &SecretRef::new("probe-key").unwrap(),
            &SecretString::new("fixture-only"),
        )
        .unwrap();
        let manager = ConnectionManager::new(shared.clone(), keys).with_probe_factory(factory);
        Self {
            _dir: dir,
            shared,
            manager,
            after_edit,
        }
    }

    /// The late answer must leave the intervening writer's exact saved bytes intact.
    fn assert_preserved(&self) {
        assert_eq!(
            std::fs::read(self.shared.path()).unwrap(),
            *self.after_edit.lock().unwrap().as_ref().unwrap()
        );
    }
}

/// An answer captured at the first endpoint cannot be saved under the second endpoint.
#[test]
fn changed_endpoint_keeps_new_models_and_returns_a_stale_error() {
    let w = World::new(|s| {
        s.connections[0].endpoint = Some("https://second.example.invalid/v1".into());
        s.connections[0].models = vec![model("new-destination-model")];
    });
    let result = w.manager.test_saved(&connection().id, true);
    assert!(
        result.is_err(),
        "a changed connection must ask to be tested again"
    );
    assert_eq!(result.unwrap_err().kind, ErrorKind::InvalidRequest);
    w.assert_preserved();
}

/// Stable saved connections still remember the service's reported models.
#[test]
fn unchanged_connection_remembers_the_reported_models() {
    let w = World::new(|_| {});
    let report = w.manager.test_saved(&connection().id, true).unwrap();
    assert_eq!(report.models, vec![model("reported-old")]);
    assert_eq!(
        w.shared.load().unwrap().connections[0].models,
        report.models
    );
}

/// A probe requested without remembering cannot replace newer settings.
#[test]
fn a_probe_without_remembering_keeps_intervening_changes() {
    let w = World::new(|s| {
        s.connections[0].endpoint = Some("https://second.example.invalid/v1".into())
    });
    w.manager.test_saved(&connection().id, false).unwrap();
    w.assert_preserved();
}

/// Identity, credentials, metadata and saved-model edits all invalidate an in-flight writeback.
#[test]
fn every_changed_connection_field_prevents_writeback() {
    for field in [
        "provider", "auth", "header", "billing", "enabled", "label", "models", "locality",
    ] {
        let w = World::new(move |s| {
            let c = &mut s.connections[0];
            match field {
                "provider" => c.provider = "replacement-provider".into(),
                "auth" => {
                    c.auth = Auth::ApiKey {
                        secret: SecretRef::new("replacement-key").unwrap(),
                    }
                }
                "header" => c.key_header = Some("x-api-key".into()),
                "billing" => c.billing = Billing::Metered,
                "enabled" => c.enabled = false,
                "label" => c.label = "Edited name".into(),
                "models" => c.models = vec![model("user-edited-model")],
                "locality" => {
                    c.locality = Locality::Local;
                    c.endpoint = Some("http://127.0.0.1:9/v1".into());
                }
                _ => unreachable!(),
            }
        });
        assert!(
            w.manager.test_saved(&connection().id, true).is_err(),
            "{field} changed during the probe"
        );
        w.assert_preserved();
    }
}

/// Deleting a tested connection cannot resurrect it through the late answer.
#[test]
fn deleted_connection_is_not_recreated() {
    let w = World::new(|s| s.connections.clear());
    assert!(w.manager.test_saved(&connection().id, true).is_err());
    w.assert_preserved();
}

/// Reusing an id for different settings cannot attach the previous connection's model list.
#[test]
fn a_replacement_connection_with_the_same_id_keeps_its_models() {
    let w = World::new(|s| {
        s.connections.clear();
        let mut replacement = connection();
        replacement.endpoint = Some("https://replacement.example.invalid/v1".into());
        replacement.models = vec![model("replacement-model")];
        s.connections.push(replacement);
    });
    assert!(w.manager.test_saved(&connection().id, true).is_err());
    w.assert_preserved();
}

/// Editing a different connection does not discard a valid answer for the tested connection.
#[test]
fn an_unrelated_connection_edit_does_not_block_remembering() {
    let w = World::new(|s| {
        let mut other = connection();
        other.id = ConnectionId::new("other-connection").unwrap();
        s.connections.push(other);
    });
    let report = w.manager.test_saved(&connection().id, true).unwrap();
    let saved = w.shared.load().unwrap();
    assert_eq!(saved.connections[0].models, report.models);
    assert_eq!(saved.connections[1].models, vec![model("saved-before")]);
}
