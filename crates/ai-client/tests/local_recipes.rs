//! Setting up one named local model of a tool that offers several.

use std::sync::Arc;

use craft_ai_client::{AiClient, ToolDesk};
use craft_ai_core::{AppId, StoreFile};
use craft_ai_features::Feature;
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;

const URL: &str = "http://127.0.0.1:8000";

/// Select subject with a second, runnable recipe after SAM 3.
fn two_models() -> Feature {
    let mut f = craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    let mut other = f.locals[0].clone();
    other.label = "Other".into();
    other.workflow = "other-segment".into();
    f.locals.push(other);
    f
}

struct World {
    _dir: tempfile::TempDir,
    shared: SharedStore,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let shared = SharedStore::new(dir.path().join("connections.json"));
        shared.replace(&StoreFile::default()).unwrap();
        Self { _dir: dir, shared }
    }

    fn desk(&self) -> ToolDesk {
        let client = AiClient::new(
            AppId::new("launcher").unwrap(),
            self.shared.clone(),
            Arc::new(MemoryStore::new()),
        );
        ToolDesk::new(client, self.shared.clone(), None)
    }

    fn nothing_written(&self) -> bool {
        let store = self.shared.load().unwrap();
        store.connections.is_empty() && store.defaults.is_empty()
    }
}

#[test]
fn a_named_second_model_becomes_its_own_connection_and_the_tool_uses_it() {
    let w = World::new();
    let done = w
        .desk()
        .enable_local_recipe(&two_models(), "other-segment", URL)
        .unwrap();
    assert_eq!(
        done.connection.as_str(),
        "local-select-subject-other-segment"
    );
    assert_eq!(done.model.as_str(), "other-segment");
    assert!(done.bound);
    let store = w.shared.load().unwrap();
    assert_eq!(store.connections.len(), 1);
    assert_eq!(store.connections[0].label, "Other on this computer");
    assert_eq!(store.defaults[0].model.as_str(), "other-segment");
}

#[test]
fn the_first_model_keeps_the_plain_connection_id_an_earlier_choice_points_at() {
    let w = World::new();
    let done = w
        .desk()
        .enable_local_recipe(&two_models(), "sam3-segment", URL)
        .unwrap();
    assert_eq!(done.connection.as_str(), "local-select-subject");
    assert_eq!(done.model.as_str(), "sam3-segment");
}

#[test]
fn two_models_of_one_tool_sit_side_by_side_and_the_first_choice_is_kept() {
    let w = World::new();
    let desk = w.desk();
    let f = two_models();
    desk.enable_local_recipe(&f, "sam3-segment", URL).unwrap();
    let second = desk.enable_local_recipe(&f, "other-segment", URL).unwrap();
    assert!(!second.bound, "a choice already made is left alone");
    let store = w.shared.load().unwrap();
    assert_eq!(store.connections.len(), 2);
    assert_eq!(store.defaults.len(), 1);
    assert_eq!(store.defaults[0].model.as_str(), "sam3-segment");
}

#[test]
fn an_unknown_model_name_is_refused_and_nothing_is_written() {
    let w = World::new();
    let err = w
        .desk()
        .enable_local_recipe(&two_models(), "no-such-segment", URL)
        .unwrap_err();
    assert!(err.message.contains("no-such-segment"), "{}", err.message);
    assert!(w.nothing_written());
}

#[test]
fn a_named_model_that_cannot_run_yet_is_refused_and_nothing_is_written() {
    let w = World::new();
    let mut f = two_models();
    f.locals[1].usable = false;
    assert!(
        w.desk()
            .enable_local_recipe(&f, "other-segment", URL)
            .is_err()
    );
    assert!(w.nothing_written());
}

#[test]
fn enabling_without_a_name_takes_the_first_model_that_can_run() {
    let w = World::new();
    let mut f = two_models();
    f.locals[0].usable = false;
    let done = w.desk().enable_local(&f, URL).unwrap();
    assert_eq!(done.model.as_str(), "other-segment");
    assert_eq!(
        done.connection.as_str(),
        "local-select-subject-other-segment"
    );
}
