//! A tool's local model as a connection an app can read.

use std::sync::Arc;

use craft_ai_client::{AiClient, GroupState, RoleStatus, SetupLink, ToolDesk};
use craft_ai_core::{
    AppId, Binding, CapabilityId, CapabilitySpec, Connection, ConnectionId, InputRole, Locality,
    ModelId, ModelInfo, Role, StoreFile,
};
use craft_ai_features::Feature;
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;
use serde_json::json;

const SAM3_URL: &str = "http://127.0.0.1:8000";

/// The built-in feature called `id`.
fn feature(id: &str) -> Feature {
    craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == id)
        .unwrap()
}

/// The role of the `select-subject` feature.
fn role() -> Role {
    feature("select-subject").role().unwrap()
}

struct World {
    _dir: tempfile::TempDir,
    shared: SharedStore,
}

impl World {
    /// An empty shared store in a temporary folder.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let shared = SharedStore::new(dir.path().join("connections.json"));
        shared.replace(&StoreFile::default()).unwrap();
        Self { _dir: dir, shared }
    }

    /// An `AiClient` for the app `app` over the shared store of the test world.
    fn client(&self, app: &str) -> AiClient {
        AiClient::new(
            AppId::new(app).unwrap(),
            self.shared.clone(),
            Arc::new(MemoryStore::new()),
        )
    }

    /// A `ToolDesk` for the app `app` over the world's shared store.
    fn desk(&self, app: &str) -> ToolDesk {
        ToolDesk::new(self.client(app), self.shared.clone(), None)
    }
}

/// Enabling the local model writes one free, keyless, loopback ComfyUI connection under the
/// plain id, offering the first recipe (SAM 2.1) for the tool with the inputs the tool sends.
#[test]
fn enabling_the_local_model_writes_a_local_connection_offering_it_for_the_tool() {
    let w = World::new();
    let done = w
        .desk("launcher")
        .enable_local(&feature("select-subject"), SAM3_URL)
        .unwrap();
    assert_eq!(done.connection.as_str(), "local-select-subject");

    let store = w.shared.load().unwrap();
    assert_eq!(store.connections.len(), 1);
    let c = &store.connections[0];
    assert_eq!(c.locality, Locality::Local);
    assert_eq!(c.endpoint.as_deref(), Some(SAM3_URL));
    assert!(c.auth.secret_ref().is_none(), "a local model needs no key");
    assert_eq!(c.provider, "comfyui");
    assert_eq!(c.billing, craft_ai_core::Billing::Free);
    assert!(c.enabled);
    // The first recipe, SAM 2.1, is the one a plain call registers.
    let model = c.model(&done.model).unwrap();
    assert_eq!(model.label, "SAM 2.1");
    // The model takes what the tool sends, and says it does this job.
    let spec = model
        .spec_for(&CapabilityId::parse("image.segment").unwrap())
        .unwrap();
    assert!(spec.accepts.contains_key(&InputRole::Source));
    assert!(spec.accepts.contains_key(&InputRole::Prompt));
}

#[test]
fn it_binds_the_tool_for_every_app_and_the_apps_read_the_address() {
    let w = World::new();
    let done = w
        .desk("launcher")
        .enable_local(&feature("select-subject"), SAM3_URL)
        .unwrap();
    assert!(done.bound);
    for app in ["photocraft", "lightcraft"] {
        let client = w.client(app);
        assert!(
            matches!(client.status(&role()), RoleStatus::Ready(_)),
            "{app}"
        );
        assert_eq!(
            client.endpoint_for(&role()).unwrap().as_deref(),
            Some(SAM3_URL),
            "{app}"
        );
    }
    assert!(w.shared.load().unwrap().apps.is_empty(), "no per-app setup");
}

#[test]
fn a_choice_a_person_already_made_is_left_alone() {
    let w = World::new();
    // Another model the person picked for the tool earlier.
    let other: Connection = serde_json::from_value(json!({
        "id": "mine", "label": "Mine", "provider": "comfyui",
        "endpoint": "http://127.0.0.1:9", "locality": "local",
        "auth": {"kind": "none"}, "billing": "free",
        "models": [ModelInfo::new(ModelId::new("yolo").unwrap(), "YOLO").with(
            CapabilitySpec::new(CapabilityId::parse("image.segment").unwrap())
                .accepting(InputRole::Source, 1)
                .accepting(InputRole::Prompt, 1),
        )],
    }))
    .unwrap();
    w.shared
        .update(|s| {
            s.connections.push(other);
            s.defaults.push(Binding {
                role: role().id,
                connection: ConnectionId::new("mine").unwrap(),
                model: ModelId::new("yolo").unwrap(),
            });
        })
        .unwrap();

    let done = w
        .desk("launcher")
        .enable_local(&feature("select-subject"), SAM3_URL)
        .unwrap();
    assert!(!done.bound);
    let store = w.shared.load().unwrap();
    assert_eq!(store.defaults.len(), 1);
    assert_eq!(store.defaults[0].connection.as_str(), "mine");
    assert_eq!(
        store.connections.len(),
        2,
        "SAM 3 is available to pick, not forced"
    );
}

/// Enabling the same local tool twice leaves one connection and one default; enabling it at a
/// new address moves the app's endpoint to that address.
#[test]
fn enabling_twice_changes_nothing_and_a_moved_address_is_updated() {
    let w = World::new();
    let desk = w.desk("launcher");
    desk.enable_local(&feature("select-subject"), SAM3_URL)
        .unwrap();
    desk.enable_local(&feature("select-subject"), SAM3_URL)
        .unwrap();
    let store = w.shared.load().unwrap();
    assert_eq!(store.connections.len(), 1);
    assert_eq!(store.defaults.len(), 1);

    desk.enable_local(&feature("select-subject"), "http://127.0.0.1:8188/")
        .unwrap();
    assert_eq!(
        w.client("photocraft")
            .endpoint_for(&role())
            .unwrap()
            .as_deref(),
        Some("http://127.0.0.1:8188")
    );
    assert_eq!(w.shared.load().unwrap().connections.len(), 1);
}

/// `enable_local` refuses a non-local address, a tool with no local recipe and a tool whose
/// recipes cannot run yet, and a refusal writes nothing to the shared store.
#[test]
fn only_a_local_address_and_only_a_runnable_local_recipe_are_accepted() {
    let w = World::new();
    let desk = w.desk("launcher");
    for bad in [
        "https://example.com",
        "http://example.com:8000",
        "http://192.168.1.5:8000",
        "http://127.0.0.1:8000/?x=1",
        "nonsense",
    ] {
        assert!(
            desk.enable_local(&feature("select-subject"), bad).is_err(),
            "{bad}"
        );
    }
    // A tool with no local recipe cannot be enabled locally.
    assert!(
        desk.enable_local(&feature("generative-fill"), SAM3_URL)
            .is_err()
    );
    // A tool none of whose recipes can run yet is not offered as ready.
    let mut later = feature("select-subject");
    for local in &mut later.locals {
        local.usable = false;
    }
    assert!(desk.enable_local(&later, SAM3_URL).is_err());
    let store = w.shared.load().unwrap();
    assert!(
        store.connections.is_empty() && store.defaults.is_empty(),
        "nothing written"
    );
}

/// After SAM 2.1 is enabled the picker lists it as ready, still offers SAM 3 to set up, and lists
/// each built-in engine as its own option, usable only where this app has that engine.
#[test]
fn the_picker_then_lists_sam_2_1_as_ready_not_as_something_to_install() {
    let w = World::new();
    w.desk("launcher")
        .enable_local(&feature("select-subject"), SAM3_URL)
        .unwrap();
    let options = craft_ai_features::setup_options(&craft_ai_features::builtin());
    let view = w.client("photocraft").picker(&role(), &options).unwrap();
    // SAM 2.1 is ready; SAM 3 is not installed, so it keeps its way in; the two built-in engines
    // (rten for SAM 2.1, Candle for SAM 3) are separate options.
    assert_eq!(view.groups.len(), 4);
    let comfy = |state| {
        view.groups
            .iter()
            .find(|g| g.state == state && !g.title.contains("built in"))
            .unwrap()
    };
    assert_eq!(comfy(GroupState::Ready).title, "SAM 2.1 on this computer");
    assert!(comfy(GroupState::Ready).models[0].selected);
    assert_eq!(
        comfy(GroupState::NotSetUp).setup,
        Some(SetupLink::New {
            option: "local:select-subject:sam3-segment".into()
        })
    );
    // With an engine's adapter built in it can be set up; without it, it is only listed.
    for (title, built) in [
        ("SAM 2.1, built in", cfg!(feature = "rten")),
        ("SAM 3, built in", cfg!(feature = "candle")),
    ] {
        let native = view
            .groups
            .iter()
            .find(|g| g.title.starts_with(title))
            .unwrap();
        if built {
            assert_eq!(native.state, GroupState::NotSetUp, "{title}");
            assert!(native.setup.is_some(), "{title}");
        } else {
            assert_eq!(native.state, GroupState::NeedsAdapter, "{title}");
            assert_eq!(native.setup, None, "{title}");
        }
    }
    assert!(view.current.unwrap().problem.is_none());
}

/// With nothing bound, an app gets no address and no error.
#[test]
fn with_nothing_bound_an_app_gets_no_address() {
    let w = World::new();
    assert_eq!(w.client("photocraft").endpoint_for(&role()).unwrap(), None);
}

/// A store saved by an earlier build, when SAM 3 was the first recipe, holds SAM 3 under the plain
/// `local-select-subject` id with the tool bound to it. Enabling SAM 2.1, first now, must not take
/// that id: the person's SAM 3 choice keeps working and SAM 2.1 gets a connection of its own.
/// Enabling SAM 3 again updates its old connection instead of adding a second one.
#[test]
fn an_earlier_sam_3_choice_on_the_plain_id_survives_sam_2_1_coming_first() {
    let w = World::new();
    let desk = w.desk("launcher");
    let mut earlier = feature("select-subject");
    earlier.locals.retain(|r| r.workflow == "sam3-segment");
    let old = desk.enable_local(&earlier, SAM3_URL).unwrap();
    assert_eq!(old.connection.as_str(), "local-select-subject");
    let f = feature("select-subject");
    let sam21 = desk.enable_local(&f, SAM3_URL).unwrap();
    assert_eq!(
        sam21.connection.as_str(),
        "local-select-subject-recipe-sam21-segment"
    );
    assert!(!sam21.bound, "the earlier choice stays");
    let again = desk
        .enable_local_recipe(&f, "sam3-segment", SAM3_URL)
        .unwrap();
    assert_eq!(again.connection.as_str(), "local-select-subject");
    let store = w.shared.load().unwrap();
    assert_eq!(store.connections.len(), 2);
    let plain = store
        .connections
        .iter()
        .find(|c| c.id.as_str() == "local-select-subject")
        .unwrap();
    assert!(
        plain
            .model(&ModelId::new("sam3-segment").unwrap())
            .is_some()
    );
    let route = w
        .client("photocraft")
        .local_route_for(&role())
        .unwrap()
        .unwrap();
    assert_eq!(
        (route.connection.as_str(), route.model.as_str()),
        ("local-select-subject", "sam3-segment")
    );
}
