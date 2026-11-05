//! A build must not lose what a newer build saved: fields it has no type for survive a rewrite at
//! every depth of the shared file, while anything the writer changed or removed stays that way.

use craft_ai_core::{ModelInfo, StoreFile};
use craft_ai_native::shared::SharedStore;
use serde_json::{Value, json};

/// A store as a newer build would save it, with a `from_newer` field at every depth.
fn newer_file() -> Value {
    json!({
        "schema_version": 1,
        "from_newer": "top",
        "connections": [
            {
                "id": "one",
                "label": "One",
                "provider": "openai",
                "endpoint": "https://one.example.com/v1",
                "locality": "remote",
                "auth": {"kind": "api_key", "secret": "one-key", "from_newer": "auth"},
                "billing": "metered",
                "from_newer": "connection one",
                "models": [
                    {
                        "id": "m1",
                        "label": "M1",
                        "from_newer": "model m1",
                        "capabilities": [
                            {"capability": "image.generate", "accepts": {}, "from_newer": "capability"}
                        ]
                    },
                    {"id": "m2", "label": "M2", "from_newer": "model m2"}
                ]
            },
            {
                "id": "two",
                "label": "Two",
                "provider": "openai",
                "endpoint": "https://two.example.com/v1",
                "locality": "remote",
                "auth": {"kind": "api_key", "secret": "two-key"},
                "billing": "metered",
                "from_newer": "connection two",
                "models": []
            }
        ],
        "defaults": [
            {"role": "fill", "connection": "one", "model": "m1", "from_newer": "default"}
        ],
        "apps": {
            "photocraft": {
                "from_newer": "app",
                "overrides": [
                    {"role": "fill", "connection": "one", "model": "m2", "from_newer": "override"}
                ]
            }
        },
        "custom_tools": [
            {"id": "my.tool", "label": "My tool", "capability": "image.generate", "inputs": ["prompt"], "from_newer": "tool"}
        ]
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

/// An unrelated write keeps every `from_newer` field, at the top, in each list item and in nested
/// objects.
#[test]
fn a_write_keeps_unknown_fields_at_every_depth() {
    let dir = tempfile::tempdir().unwrap();
    let store = saved(&dir, &newer_file());
    store
        .update(|file: &mut StoreFile| file.model_folders.clear())
        .unwrap();
    let after = on_disk(&store);
    assert_eq!(after["from_newer"], "top");
    let one = &after["connections"][0];
    assert_eq!(one["from_newer"], "connection one");
    assert_eq!(one["auth"]["from_newer"], "auth");
    assert_eq!(one["models"][0]["from_newer"], "model m1");
    assert_eq!(
        one["models"][0]["capabilities"][0]["from_newer"],
        "capability"
    );
    assert_eq!(one["models"][1]["from_newer"], "model m2");
    assert_eq!(after["connections"][1]["from_newer"], "connection two");
    assert_eq!(after["defaults"][0]["from_newer"], "default");
    assert_eq!(after["apps"]["photocraft"]["from_newer"], "app");
    assert_eq!(
        after["apps"]["photocraft"]["overrides"][0]["from_newer"],
        "override"
    );
    assert_eq!(after["custom_tools"][0]["from_newer"], "tool");
}

/// Changing a known field of an item keeps that item's unknown field beside the change.
#[test]
fn a_kept_binding_keeps_its_unknown_field_when_its_model_changes() {
    let dir = tempfile::tempdir().unwrap();
    let store = saved(&dir, &newer_file());
    store
        .update(|file| file.defaults[0].model = "m2".parse().unwrap())
        .unwrap();
    let after = on_disk(&store);
    assert_eq!(
        after["defaults"][0]["model"], "m2",
        "the change was written"
    );
    assert_eq!(after["defaults"][0]["from_newer"], "default");
}

/// Reordering connections and models keeps each item's unknown field with that item.
#[test]
fn items_are_matched_by_id_not_by_position() {
    let dir = tempfile::tempdir().unwrap();
    let store = saved(&dir, &newer_file());
    store
        .update(|file| {
            file.connections.reverse();
            file.connections[1].models.reverse();
        })
        .unwrap();
    let after = on_disk(&store);
    assert_eq!(after["connections"][0]["id"], "two");
    assert_eq!(after["connections"][0]["from_newer"], "connection two");
    assert_eq!(after["connections"][1]["from_newer"], "connection one");
    assert_eq!(after["connections"][1]["models"][0]["id"], "m2");
    assert_eq!(
        after["connections"][1]["models"][0]["from_newer"],
        "model m2"
    );
}

/// Removing items removes their unknown fields too; only what stayed keeps its field.
#[test]
fn a_removed_item_takes_its_unknown_fields_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = saved(&dir, &newer_file());
    store
        .update(|file| {
            file.defaults.clear();
            file.apps.clear();
            file.connections.retain(|c| c.id.as_str() == "one");
            file.connections[0].models.retain(|m| m.id.as_str() == "m1");
        })
        .unwrap();
    let text = std::fs::read_to_string(store.path()).unwrap();
    for gone in [
        "connection two",
        "model m2",
        "\"default\"",
        "\"app\"",
        "override",
    ] {
        assert!(
            !text.contains(gone),
            "{gone} should have gone with its item"
        );
    }
    assert!(text.contains("model m1"), "what stayed keeps its field");
}

/// A field this build knows and clears stays cleared; the old file does not bring it back.
#[test]
fn a_field_this_build_knows_can_still_be_cleared() {
    let dir = tempfile::tempdir().unwrap();
    let mut file = newer_file();
    file["connections"][0]["models"][0]["tags"] = json!(["recommended"]);
    file["connections"][0]["models"][0]["local_file"] = json!("C:/models/m1.onnx");
    let store = saved(&dir, &file);
    store
        .update(|file| {
            let model = &mut file.connections[0].models[0];
            model.tags.clear();
            model.local_file = None;
        })
        .unwrap();
    let after = on_disk(&store);
    let model = &after["connections"][0]["models"][0];
    assert!(
        model.get("tags").is_none(),
        "a cleared field must not come back"
    );
    assert!(
        model.get("local_file").is_none(),
        "a cleared field must not come back"
    );
}

/// An item the writer adds gets no unknown fields from anywhere.
#[test]
fn a_new_item_is_written_as_given() {
    let dir = tempfile::tempdir().unwrap();
    let store = saved(&dir, &newer_file());
    store
        .update(|file| {
            file.connections[0]
                .models
                .push(ModelInfo::new("m3".parse().unwrap(), "M3"));
        })
        .unwrap();
    let after = on_disk(&store);
    let added = &after["connections"][0]["models"][2];
    assert_eq!(added["id"], "m3");
    assert!(added.get("from_newer").is_none());
}

/// Two file items with the same role are both left as written, since either could be the source.
#[test]
fn an_item_whose_id_appears_twice_is_not_guessed_at() {
    let dir = tempfile::tempdir().unwrap();
    let mut file = newer_file();
    file["defaults"] = json!([
        {"role": "fill", "connection": "one", "model": "m1", "from_newer": "first"},
        {"role": "fill", "connection": "one", "model": "m2", "from_newer": "second"}
    ]);
    let store = saved(&dir, &file);
    store.update(|file| file.model_folders.clear()).unwrap();
    let after = on_disk(&store);
    assert_eq!(after["defaults"].as_array().unwrap().len(), 2);
    assert!(after["defaults"][0].get("from_newer").is_none());
    assert!(after["defaults"][1].get("from_newer").is_none());
}

/// `replace` with the loaded store keeps the unknown fields of the old file.
#[test]
fn replace_keeps_unknown_fields_of_items_it_still_holds() {
    let dir = tempfile::tempdir().unwrap();
    let store = saved(&dir, &newer_file());
    let file = store.load().unwrap();
    store.replace(&file).unwrap();
    let after = on_disk(&store);
    assert_eq!(after["from_newer"], "top");
    assert_eq!(
        after["connections"][0]["models"][0]["from_newer"],
        "model m1"
    );
}

/// A second write still keeps the fields the first write carried over.
#[test]
fn two_writes_in_a_row_keep_the_fields() {
    let dir = tempfile::tempdir().unwrap();
    let store = saved(&dir, &newer_file());
    store.update(|file| file.model_folders.clear()).unwrap();
    store
        .update(|file| file.connections[0].label = "Renamed".to_owned())
        .unwrap();
    let after = on_disk(&store);
    assert_eq!(after["connections"][0]["label"], "Renamed");
    assert_eq!(
        after["connections"][0]["models"][1]["from_newer"],
        "model m2"
    );
}
