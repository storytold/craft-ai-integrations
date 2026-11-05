//! List items are matched on the identity this build writes, and a written identity that appears
//! twice is ambiguous. A newer build may add an `id` to an item this build knows only by `role`, and
//! a writer may add a second item with an identity the file already holds.

use craft_ai_core::{Binding, StoreFile};
use craft_ai_native::shared::SharedStore;
use serde_json::{Value, json};

/// A store whose one default carries an `id` this build has no field for, plus a newer field.
fn file_with_binding(binding: Value) -> Value {
    json!({
        "schema_version": 1,
        "connections": [
            {
                "id": "one",
                "label": "One",
                "provider": "openai",
                "endpoint": "https://one.example.com/v1",
                "locality": "remote",
                "auth": {"kind": "api_key", "secret": "one-key"},
                "billing": "metered",
                "models": [{"id": "m1", "label": "M1"}, {"id": "m2", "label": "M2"}]
            }
        ],
        "defaults": [binding]
    })
}

/// Saves `file` as the shared store in `dir` and returns the store.
fn saved(dir: &tempfile::TempDir, file: &Value) -> SharedStore {
    let store = SharedStore::new(dir.path().join("connections.json"));
    std::fs::write(store.path(), file.to_string()).unwrap();
    store
}

/// Reads the shared store back as raw JSON.
fn on_disk(store: &SharedStore) -> Value {
    serde_json::from_slice(&std::fs::read(store.path()).unwrap()).unwrap()
}

/// A binding a newer build gave an `id` keeps its unknown fields, because this build writes it with
/// `role` only and the match must use that.
#[test]
fn an_item_with_an_unknown_id_is_matched_by_the_identity_this_build_writes() {
    let dir = tempfile::tempdir().unwrap();
    let store = saved(
        &dir,
        &file_with_binding(json!({
            "id": "binding-7",
            "role": "fill",
            "connection": "one",
            "model": "m1",
            "from_newer": "default"
        })),
    );
    store
        .update(|file: &mut StoreFile| file.model_folders.clear())
        .unwrap();
    let after = on_disk(&store);
    assert_eq!(after["defaults"][0]["id"], "binding-7");
    assert_eq!(after["defaults"][0]["from_newer"], "default");
}

/// A writer that adds a second default for a role already in the file leaves both as written:
/// copying the old item's unknown fields onto either would be a guess.
#[test]
fn an_identity_written_twice_is_not_guessed_at() {
    let dir = tempfile::tempdir().unwrap();
    let store = saved(
        &dir,
        &file_with_binding(json!({
            "role": "fill",
            "connection": "one",
            "model": "m1",
            "from_newer": "default"
        })),
    );
    store
        .update(|file: &mut StoreFile| {
            file.defaults.push(Binding {
                role: "fill".parse().unwrap(),
                connection: "one".parse().unwrap(),
                model: "m2".parse().unwrap(),
            });
        })
        .unwrap();
    let after = on_disk(&store);
    let defaults = after["defaults"].as_array().unwrap();
    assert_eq!(defaults.len(), 2);
    assert!(defaults[0].get("from_newer").is_none());
    assert!(defaults[1].get("from_newer").is_none());
}

/// When the file holds an identity twice and the writer keeps only one of those items, the kept item
/// still gets nothing: which old item it came from cannot be told, so copying either would be a guess.
#[test]
fn an_identity_the_file_holds_twice_is_not_guessed_at_when_written_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut file = file_with_binding(json!({
        "role": "fill", "connection": "one", "model": "m1", "from_newer": "first"
    }));
    file["defaults"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role": "fill", "connection": "one", "model": "m2", "from_newer": "second"}));
    let store = saved(&dir, &file);
    store
        .update(|file: &mut StoreFile| {
            file.defaults.remove(0);
        })
        .unwrap();
    let after = on_disk(&store);
    let defaults = after["defaults"].as_array().unwrap();
    assert_eq!(defaults.len(), 1);
    assert_eq!(defaults[0]["model"], "m2");
    assert!(defaults[0].get("from_newer").is_none());
}
