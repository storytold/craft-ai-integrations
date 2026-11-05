//! Setting up one named local model of a tool that offers several.

use std::sync::Arc;

use craft_ai_client::{AiClient, SetupLink, ToolDesk};
use craft_ai_core::{AppId, StoreFile};
use craft_ai_features::{Feature, Gpu, GpuVendor, NativeRoute, Os, SystemProfile, setup_options};
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;

const URL: &str = "http://127.0.0.1:8000";

/// Select subject with a second, runnable recipe after SAM 3.
fn two_models() -> Feature {
    let mut f = craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    // Start from SAM 3 alone, found by its workflow, whatever else the catalogue lists.
    f.locals.retain(|r| r.workflow == "sam3-segment");
    assert_eq!(f.locals.len(), 1);
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

    fn weights(&self) -> std::path::PathBuf {
        let p = self._dir.path().join("weights.safetensors");
        std::fs::write(&p, b"not really weights").unwrap();
        p
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
        "local-select-subject-recipe-other-segment"
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
        "local-select-subject-recipe-other-segment"
    );
}

/// The first recipe can't run yet; the second can, and lists only the `burn` runtime, so a
/// runtime taken from the wrong recipe shows.
fn only_the_second_runs() -> Feature {
    let mut f = two_models();
    f.locals[0].usable = false;
    f.locals[1].native.retain(|n| n.provider == "burn");
    f
}

/// A recipe named like a built-in runtime (`candle`) gets its own connection id, so enabling it
/// leaves the runtime's connection and its binding alone.
#[test]
fn a_recipe_named_like_a_built_in_runtime_never_takes_its_connection() {
    let w = World::new();
    let desk = w.desk();
    let mut f = two_models();
    f.locals[1].workflow = "candle".into();
    let native = desk.enable_native_with(&f, "candle", &w.weights()).unwrap();
    let recipe = desk.enable_local_recipe(&f, "candle", URL).unwrap();
    assert_ne!(native.connection, recipe.connection);
    let store = w.shared.load().unwrap();
    assert_eq!(store.connections.len(), 2, "neither replaced the other");
    let bound = &store.defaults[0];
    assert_eq!(bound.connection, native.connection);
    let kept = store
        .connections
        .iter()
        .find(|c| c.id == bound.connection)
        .unwrap();
    assert_eq!(
        kept.provider, "candle",
        "the binding still points at the runtime it chose"
    );
}

/// Engine advice comes from the first recipe that can run, not from the first recipe listed.
#[test]
fn engine_advice_follows_the_first_recipe_that_can_run() {
    let w = World::new();
    let client = AiClient::new(
        AppId::new("launcher").unwrap(),
        w.shared.clone(),
        Arc::new(MemoryStore::new()),
    );
    let profile = SystemProfile {
        os: Os::Windows,
        gpus: vec![Gpu {
            vendor: GpuVendor::Nvidia,
            name: "RTX 3090 Ti".to_owned(),
            vram_mb: Some(24_576),
        }],
        comfyui_found: true,
    };
    assert!(
        !client
            .engine_advice(&only_the_second_runs(), &profile)
            .is_empty()
    );
}

/// With the first recipe unable to run, the built-in runtime comes from the second recipe, and a
/// runtime only the first one lists is refused with nothing written.
#[test]
fn a_built_in_runtime_comes_from_the_first_recipe_that_can_run() {
    let w = World::new();
    let f = only_the_second_runs();
    let first = w.desk().enable_native(&f, &w.weights()).unwrap();
    assert_eq!(first.connection.as_str(), "local-select-subject-burn");
    // Only the runnable recipe's runtimes count: the first recipe's `candle` is not offered.
    assert!(
        w.desk()
            .enable_native_with(&f, "candle", &w.weights())
            .is_err()
    );
    assert_eq!(w.shared.load().unwrap().connections.len(), 1);
}

/// The setup links the picker offers for `f`'s local models, as option ids, in catalogue order.
/// Built-in engine links are left out: with an engine's cargo feature on, they appear too.
fn setup_links(w: &World, f: &Feature) -> Vec<String> {
    let client = AiClient::new(
        AppId::new("launcher").unwrap(),
        w.shared.clone(),
        Arc::new(MemoryStore::new()),
    );
    let options = setup_options(std::slice::from_ref(f));
    let view = client.picker(&f.role().unwrap(), &options).unwrap();
    view.groups
        .iter()
        .filter_map(|g| match &g.setup {
            Some(SetupLink::New { option }) if option.starts_with("local:") => Some(option.clone()),
            _ => None,
        })
        .collect()
}

/// Enabling one recipe removes only its own setup link from the picker; the other model, not
/// installed yet, keeps its link until it is enabled too.
#[test]
fn enabling_one_model_keeps_the_setup_link_of_the_other() {
    let w = World::new();
    let f = two_models();
    assert_eq!(
        setup_links(&w, &f),
        [
            "local:select-subject:sam3-segment",
            "local:select-subject:other-segment"
        ]
    );
    w.desk()
        .enable_local_recipe(&f, "sam3-segment", URL)
        .unwrap();
    assert_eq!(
        setup_links(&w, &f),
        ["local:select-subject:other-segment"],
        "the model that is not installed keeps its way in"
    );
    w.desk()
        .enable_local_recipe(&f, "other-segment", URL)
        .unwrap();
    assert!(
        setup_links(&w, &f).is_empty(),
        "both installed, nothing to set up"
    );
}

/// Enabling only the second recipe leaves the first recipe's setup link in the picker.
#[test]
fn enabling_the_second_model_alone_leaves_the_first_one_to_set_up() {
    let w = World::new();
    let f = two_models();
    w.desk()
        .enable_local_recipe(&f, "other-segment", URL)
        .unwrap();
    assert_eq!(setup_links(&w, &f), ["local:select-subject:sam3-segment"]);
}

/// The first recipe lists a custom runtime whose name is the second recipe's connection suffix.
fn runtime_named_like_a_recipe() -> Feature {
    let mut f = two_models();
    f.locals[0].native.push(NativeRoute {
        provider: "recipe-other-segment".into(),
        model: "sam3".into(),
        label: "Custom".into(),
        models: Vec::new(),
        note: None,
        fast_on_processor: false,
    });
    f
}

/// Enabling a recipe whose connection id a custom runtime already holds is refused, and the
/// store (connection and binding) is exactly as it was.
#[test]
fn a_recipe_never_replaces_a_custom_runtime_that_has_its_connection_id() {
    let w = World::new();
    let desk = w.desk();
    let f = runtime_named_like_a_recipe();
    let native = desk
        .enable_native_with(&f, "recipe-other-segment", &w.weights())
        .unwrap();
    let before = w.shared.load().unwrap();
    let err = desk
        .enable_local_recipe(&f, "other-segment", URL)
        .unwrap_err();
    assert!(
        err.message.contains(native.connection.as_str()),
        "{}",
        err.message
    );
    let after = w.shared.load().unwrap();
    assert_eq!(after.connections, before.connections);
    assert_eq!(after.defaults, before.defaults);
    assert_eq!(after.connections[0].provider, "recipe-other-segment");
}

/// The same clash in the other order: a custom runtime cannot take over a recipe's connection.
#[test]
fn a_custom_runtime_never_replaces_a_recipe_that_has_its_connection_id() {
    let w = World::new();
    let desk = w.desk();
    let f = runtime_named_like_a_recipe();
    desk.enable_local_recipe(&f, "other-segment", URL).unwrap();
    let before = w.shared.load().unwrap();
    assert!(
        desk.enable_native_with(&f, "recipe-other-segment", &w.weights())
            .is_err()
    );
    let after = w.shared.load().unwrap();
    assert_eq!(after.connections, before.connections);
    assert_eq!(after.connections[0].provider, "comfyui");
}

/// Enabling a recipe twice is not a clash: the one connection is updated with the new address.
#[test]
fn enabling_the_same_recipe_again_still_updates_its_connection() {
    let w = World::new();
    let desk = w.desk();
    let f = two_models();
    desk.enable_local_recipe(&f, "other-segment", URL).unwrap();
    desk.enable_local_recipe(&f, "other-segment", "http://127.0.0.1:9000")
        .unwrap();
    let store = w.shared.load().unwrap();
    assert_eq!(store.connections.len(), 1);
    assert_eq!(
        store.connections[0].endpoint.as_deref(),
        Some("http://127.0.0.1:9000")
    );
}
