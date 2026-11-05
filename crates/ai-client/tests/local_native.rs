//! Built-in runtimes beside the ComfyUI route: the launcher registers a built-in route (rten for SAM
//! 2.1 by default, Candle for SAM 3 when asked) next to the ComfyUI one, both for the same tool, and
//! an app reads the exact file it is meant to load.
use std::path::PathBuf;
use std::sync::Arc;

use craft_ai_client::{AiClient, ToolDesk};
use craft_ai_core::{AppId, Role, StoreFile};
use craft_ai_features::Feature;
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;

const SAM3_URL: &str = "http://127.0.0.1:8000";

fn feature(id: &str) -> Feature {
    craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == id)
        .unwrap()
}

fn role() -> Role {
    feature("select-subject").role().unwrap()
}

struct World {
    dir: tempfile::TempDir,
    shared: SharedStore,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let shared = SharedStore::new(dir.path().join("connections.json"));
        shared.replace(&StoreFile::default()).unwrap();
        Self { dir, shared }
    }
    fn desk(&self, app: &str) -> ToolDesk {
        ToolDesk::new(self.client(app), self.shared.clone(), None)
    }
    fn client(&self, app: &str) -> AiClient {
        AiClient::new(
            AppId::new(app).unwrap(),
            self.shared.clone(),
            Arc::new(MemoryStore::new()),
        )
    }
    /// A weights file that exists (the content does not matter here).
    fn weights(&self, name: &str) -> PathBuf {
        let p = self.dir.path().join(name);
        std::fs::write(&p, b"not really weights").unwrap();
        p
    }
}

/// The plain call registers and binds the first recipe's first runtime, SAM 2.1 on rten, with
/// the weights file it was given, and an app reads that route with no address.
#[test]
fn the_native_route_is_registered_bound_and_carries_the_file() {
    let w = World::new();
    let file = w.weights("encoder.onnx");
    let done = w
        .desk("launcher")
        .enable_native(&feature("select-subject"), &file)
        .unwrap();
    assert!(done.bound);
    // The plain call takes the first recipe's first runtime: SAM 2.1 on rten, reading the ONNX
    // encoder file.
    assert_eq!(done.connection.as_str(), "local-select-subject-rten");
    assert_eq!(done.model.as_str(), "sam21");
    let route = w
        .client("lightcraft")
        .local_route_for(&role())
        .unwrap()
        .unwrap();
    assert_eq!(route.provider, "rten");
    assert_eq!(route.endpoint, None);
    assert_eq!(route.file.as_deref(), file.to_str());
}

/// Two apps read the one weights file Candle was registered with.
/// SAM 2.1 on rten reads published `.onnx` files: the encoder file is accepted as the weights,
/// registered under the rten connection and bound, and an app reads that exact file back.
#[test]
fn the_native_route_is_registered_for_an_onnx_file() {
    let w = World::new();
    let file = w.weights("encoder.onnx");
    let done = w
        .desk("launcher")
        .enable_native_with(&feature("select-subject"), "rten", &file)
        .unwrap();
    assert!(done.bound);
    assert_eq!(done.connection.as_str(), "local-select-subject-rten");
    assert_eq!(done.model.as_str(), "sam21");
    let route = w
        .client("lightcraft")
        .local_route_for(&role())
        .unwrap()
        .unwrap();
    assert_eq!(route.provider, "rten");
    assert_eq!(route.file.as_deref(), file.to_str());
}

/// The Burn engine reads the saved copy made once on this computer: its encoder `.bpk` file is
/// accepted as the weights, registered under the Burn connection and bound, and an app reads that
/// exact file back. A file of any other kind is still refused.
#[test]
fn the_burn_route_is_registered_for_a_prepared_bpk_file() {
    let w = World::new();
    let file = w.weights("encoder.bpk");
    let done = w
        .desk("launcher")
        .enable_native_with(&feature("select-subject"), "burn", &file)
        .unwrap();
    assert_eq!(done.connection.as_str(), "local-select-subject-burn");
    assert_eq!(done.model.as_str(), "sam21");
    let route = w
        .client("lightcraft")
        .local_route_for(&role())
        .unwrap()
        .unwrap();
    assert_eq!(route.provider, "burn");
    assert_eq!(route.file.as_deref(), file.to_str());
    for other in ["encoder.bin", "encoder.pt", "encoder"] {
        let refused = w.desk("launcher").enable_native_with(
            &feature("select-subject"),
            "burn",
            &w.weights(other),
        );
        assert!(refused.is_err(), "{other} is not a weights file");
    }
}

/// Two apps that share the store read the same registered weights file.
#[test]
fn two_apps_read_the_same_file() {
    let w = World::new();
    let file = w.weights("sam3.safetensors");
    w.desk("launcher")
        .enable_native_with(&feature("select-subject"), "candle", &file)
        .unwrap();
    for app in ["lightcraft", "photocraft"] {
        assert_eq!(
            w.client(app).local_file_for(&role(), "candle").unwrap(),
            Some(file.clone()),
            "{app}"
        );
    }
}

/// A ComfyUI choice records no weights file, and a built-in runtime lookup on it is refused in
/// plain words.
#[test]
fn the_comfyui_route_has_no_file_and_is_not_a_native_one() {
    let w = World::new();
    w.desk("launcher")
        .enable_local(&feature("select-subject"), SAM3_URL)
        .unwrap();
    let client = w.client("lightcraft");
    assert_eq!(client.local_route_for(&role()).unwrap().unwrap().file, None);
    let error = client
        .local_file_for(&role(), "candle")
        .unwrap_err()
        .message;
    assert_eq!(
        error,
        "The model you chose can't be run by this app yet. Choose another in Set up AI."
    );
}

/// An app that is bound to a built-in runtime gets the plain "can not run the model yet" refusal
/// when it asks for a ComfyUI address.
#[test]
fn a_native_route_is_not_offered_as_a_comfyui_address() {
    let w = World::new();
    w.desk("launcher")
        .enable_native(&feature("select-subject"), &w.weights("sam3.safetensors"))
        .unwrap();
    assert_eq!(
        w.client("photocraft")
            .endpoint_for(&role())
            .unwrap_err()
            .message,
        "The model you chose can't be run by this app yet. Choose another in Set up AI."
    );
}

/// An app asking for a built-in runtime file when nothing is bound gets none, not an error.
#[test]
fn nothing_bound_is_not_an_error() {
    let w = World::new();
    assert_eq!(
        w.client("lightcraft")
            .local_file_for(&role(), "candle")
            .unwrap(),
        None
    );
}

/// Enabling the built-in runtime after a ComfyUI choice leaves that choice bound and registers both
/// connections.
#[test]
fn a_choice_already_made_is_left_alone_and_both_routes_are_registered() {
    let w = World::new();
    w.desk("launcher")
        .enable_local(&feature("select-subject"), SAM3_URL)
        .unwrap();
    let done = w
        .desk("launcher")
        .enable_native(&feature("select-subject"), &w.weights("sam3.safetensors"))
        .unwrap();
    assert!(!done.bound, "the person's existing choice stays");
    assert_eq!(
        w.client("lightcraft")
            .local_route_for(&role())
            .unwrap()
            .unwrap()
            .provider,
        "comfyui"
    );
    let store = w.shared.load().unwrap();
    assert_eq!(store.connections.len(), 2);
}

/// Registering Candle again with another file updates the one connection to the new file and
/// leaves the binding as it was.
#[test]
fn registering_again_moves_the_file_and_keeps_one_connection() {
    let w = World::new();
    w.desk("launcher")
        .enable_native_with(
            &feature("select-subject"),
            "candle",
            &w.weights("old.safetensors"),
        )
        .unwrap();
    let new = w.weights("new.safetensors");
    let done = w
        .desk("launcher")
        .enable_native_with(&feature("select-subject"), "candle", &new)
        .unwrap();
    assert!(!done.bound, "already bound by the first call");
    assert_eq!(w.shared.load().unwrap().connections.len(), 1);
    assert_eq!(
        w.client("lightcraft")
            .local_file_for(&role(), "candle")
            .unwrap(),
        Some(new)
    );
}

/// A missing file, a folder, a wrong extension and a relative path are each refused and nothing is
/// registered.
#[test]
fn a_file_that_cannot_be_the_weights_is_refused_and_nothing_is_written() {
    let w = World::new();
    let desk = w.desk("launcher");
    let f = feature("select-subject");
    let missing = w.dir.path().join("nope.safetensors");
    let folder = w.dir.path().join("folder.safetensors");
    std::fs::create_dir(&folder).unwrap();
    let wrong_type = w.weights("sam3.bin");
    for (what, path) in [
        ("missing", missing),
        ("folder", folder),
        ("wrong extension", wrong_type),
        ("relative", PathBuf::from("sam3.safetensors")),
    ] {
        assert!(desk.enable_native(&f, &path).is_err(), "{what}");
    }
    assert!(w.shared.load().unwrap().connections.is_empty());
}

/// A relative weights path is refused even when a file is there, and nothing is registered.
#[test]
fn a_relative_path_is_refused_even_when_it_points_at_a_real_file() {
    let w = World::new();
    let name = format!("lc-relative-{}.safetensors", std::process::id());
    std::fs::write(&name, b"x").unwrap();
    let refused = w
        .desk("launcher")
        .enable_native(&feature("select-subject"), std::path::Path::new(&name))
        .is_err();
    std::fs::remove_file(&name).unwrap();
    assert!(refused);
    assert!(w.shared.load().unwrap().connections.is_empty());
}

/// A feature with no usable recipe, or with no built-in route, refuses the built-in runtime and
/// writes nothing.
#[test]
fn a_runtime_that_is_not_ready_to_use_is_refused() {
    let w = World::new();
    // No recipe that can run: nothing to register. (A later recipe that can run is used instead,
    // see `the_next_recipe_that_can_run_supplies_the_built_in_runtime`.)
    let mut f = feature("select-subject");
    f.locals.iter_mut().for_each(|r| r.usable = false);
    assert!(
        w.desk("launcher")
            .enable_native(&f, &w.weights("sam3.safetensors"))
            .is_err()
    );
    let mut f = feature("select-subject");
    f.locals.iter_mut().for_each(|r| r.native.clear());
    assert!(
        w.desk("launcher")
            .enable_native(&f, &w.weights("sam3.safetensors"))
            .is_err()
    );
    assert!(w.shared.load().unwrap().connections.is_empty());
}

/// With SAM 2.1 unable to run, the plain call registers SAM 3's first runtime, Candle; rten,
/// listed only by SAM 2.1, is refused, and a file of neither kind writes nothing.
#[test]
fn the_next_recipe_that_can_run_supplies_the_built_in_runtime() {
    let w = World::new();
    let mut f = feature("select-subject");
    assert_eq!(f.locals[0].workflow, "sam21-segment");
    f.locals[0].usable = false;
    // SAM 2.1 cannot run, so the built-in runtime is the first one SAM 3 lists, Candle.
    let weights = w.weights("sam3.safetensors");
    let enabled = w.desk("launcher").enable_native(&f, &weights).unwrap();
    let store = w.shared.load().unwrap();
    let connection = store
        .connections
        .iter()
        .find(|c| c.provider == "candle")
        .expect("the candle connection is saved");
    assert_eq!(connection.models[0].id.as_str(), "sam3");
    assert_eq!(
        connection.models[0].local_file.as_deref(),
        weights.to_str(),
        "{enabled:?}"
    );
    // rten is listed only by SAM 2.1, which cannot run, so it is refused.
    assert!(
        w.desk("launcher")
            .enable_native_with(&f, "rten", &w.weights("encoder.onnx"))
            .is_err()
    );
    // A file that is neither kind is still refused, and nothing more is written.
    assert!(
        w.desk("launcher")
            .enable_native(&f, &w.weights("encoder.bin"))
            .is_err()
    );
    assert_eq!(w.shared.load().unwrap().connections.len(), 1);
}

/// Every tool other than Select subject and Remove background refuses a built-in runtime and
/// writes nothing.
#[test]
fn a_tool_without_a_built_in_runtime_is_refused() {
    let w = World::new();
    let file = w.weights("sam3.safetensors");
    let others = craft_ai_features::builtin()
        .into_iter()
        .filter(|f| f.id != "select-subject" && f.id != "remove-background");
    for f in others {
        assert!(
            w.desk("launcher").enable_native(&f, &file).is_err(),
            "{}",
            f.id
        );
    }
    assert!(w.shared.load().unwrap().connections.is_empty());
}

/// A Candle weights file that has gone is an error naming that, not an unbound tool.
#[test]
fn a_file_that_vanished_is_an_error_not_unbound() {
    let w = World::new();
    let file = w.weights("sam3.safetensors");
    w.desk("launcher")
        .enable_native_with(&feature("select-subject"), "candle", &file)
        .unwrap();
    std::fs::remove_file(&file).unwrap();
    let error = w
        .client("lightcraft")
        .local_file_for(&role(), "candle")
        .unwrap_err()
        .message;
    assert_eq!(
        error,
        "The file for the model you chose is no longer there. Open Set up AI to fix it."
    );
}

/// With the ComfyUI model and a built-in runtime both registered, the picker lists them as separate
/// groups with different titles, one of them saying "built in".
#[test]
fn the_picker_offers_the_two_runtimes_of_one_model_side_by_side() {
    let w = World::new();
    w.desk("launcher")
        .enable_local(&feature("select-subject"), SAM3_URL)
        .unwrap();
    w.desk("launcher")
        .enable_native(&feature("select-subject"), &w.weights("sam3.safetensors"))
        .unwrap();
    let view = w.client("lightcraft").picker(&role(), &[]).unwrap();
    let titles: Vec<&str> = view.groups.iter().map(|g| g.title.as_str()).collect();
    assert_eq!(titles.len(), 2, "{titles:?}");
    assert_ne!(
        titles[0], titles[1],
        "the person must be able to tell them apart"
    );
    assert!(titles.iter().any(|t| t.contains("built in")), "{titles:?}");
}

#[test]
fn old_stores_without_the_field_load_and_it_is_not_written_when_absent() {
    let json = r#"{"id":"sam3","label":"SAM 3"}"#;
    let info: craft_ai_core::ModelInfo = serde_json::from_str(json).unwrap();
    assert_eq!(info.local_file, None);
    assert!(!serde_json::to_string(&info).unwrap().contains("local_file"));
}
