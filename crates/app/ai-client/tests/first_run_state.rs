//! What the first-run screens read: a built-in model whose file has gone is not ready and is listed
//! once; a route that selects by clicking says so before and after set up; a built-in engine that is
//! quick on the main chip is suggested and never called slow. Scripted only.
use std::sync::Arc;

use craft_ai_client::{AiClient, GroupState, ProviderFactory, RoleStatus, ToolDesk};
use craft_ai_core::{AiError, AppId, Role};
use craft_ai_features::{Feature, Gpu, GpuVendor, Os, Pace, SystemProfile, builtin, setup_options};
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;

/// The title every SAM 2.1 built-in row starts with.
const RTEN_ROW: &str = "SAM 2.1, built in (the processor";

/// The built-in Select subject tool.
fn select_subject() -> Feature {
    builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap()
}

/// A factory where the app has the rten engine and a Burn engine that uses Nvidia cards.
fn engines() -> ProviderFactory {
    let mut factory = ProviderFactory::standard();
    for name in ["rten", "burn"] {
        factory.register(name, |_, _| Err(AiError::unsupported("not run here")));
    }
    factory.declare_accelerators("rten", &[]);
    factory.declare_accelerators("burn", &[GpuVendor::Nvidia]);
    factory
}

/// A client on the store in `dir`, and the desk that registers models there.
fn world(dir: &tempfile::TempDir) -> (AiClient, ToolDesk) {
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let client = AiClient::new(
        AppId::new("test-app").unwrap(),
        shared.clone(),
        Arc::new(MemoryStore::new()),
    )
    .with_factory(engines());
    let launcher = AiClient::new(
        ToolDesk::app().unwrap(),
        shared.clone(),
        Arc::new(MemoryStore::new()),
    );
    (client, ToolDesk::new(launcher, shared, None))
}

/// The picker rows whose title starts with `RTEN_ROW`.
fn rten_rows(client: &AiClient, role: &Role) -> Vec<craft_ai_client::PickerGroup> {
    let options = setup_options(&builtin());
    client
        .picker(role, &options)
        .unwrap()
        .groups
        .into_iter()
        .filter(|g| g.title.starts_with(RTEN_ROW))
        .collect()
}

/// Once the weights file has gone the tool is not ready, and the model is listed once, as one to
/// set up again, never also as one that can be picked.
#[test]
fn a_built_in_model_whose_file_is_gone_is_not_ready_and_is_listed_once() {
    let dir = tempfile::tempdir().unwrap();
    let (client, desk) = world(&dir);
    let feature = select_subject();
    let role = feature.role().unwrap();
    let weights = dir.path().join("encoder.onnx");
    std::fs::write(&weights, b"weights").unwrap();
    desk.enable_native_with(&feature, "rten", &weights).unwrap();

    assert!(matches!(client.status(&role), RoleStatus::Ready(_)));
    let rows = rten_rows(&client, &role);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, GroupState::Ready);

    std::fs::remove_file(&weights).unwrap();
    match client.status(&role) {
        RoleStatus::Unavailable { reason } => assert!(reason.contains("no longer there")),
        other => panic!("expected the tool to be unavailable, got {other:?}"),
    }
    let rows = rten_rows(&client, &role);
    assert_eq!(rows.len(), 1, "the same model is not listed twice");
    assert_eq!(rows[0].state, GroupState::NotSetUp);
    assert!(rows[0].note.contains("missing"));
    let view = client.picker(&role, &setup_options(&builtin())).unwrap();
    assert!(view.current.unwrap().problem.is_some());
}

/// The route that selects by clicking says so before it is set up and after, and does not offer a
/// prompt it refuses.
#[test]
fn the_clicks_only_route_says_so_before_and_after_set_up() {
    let dir = tempfile::tempdir().unwrap();
    let (client, desk) = world(&dir);
    let feature = select_subject();
    let role = feature.role().unwrap();
    let before = rten_rows(&client, &role);
    assert_eq!(before.len(), 1);
    assert!(before[0].note.contains("clicking"), "{}", before[0].note);

    let weights = dir.path().join("encoder.onnx");
    std::fs::write(&weights, b"weights").unwrap();
    desk.enable_native_with(&feature, "rten", &weights).unwrap();
    let after = rten_rows(&client, &role);
    let description = &after[0].models[0].description;
    assert!(description.contains("clicking"), "{description}");
    assert!(!description.contains("prompt"), "{description}");
}

/// A system with a graphics card.
fn with_card() -> SystemProfile {
    SystemProfile {
        os: Os::Windows,
        gpus: vec![Gpu {
            vendor: GpuVendor::Nvidia,
            name: "RTX 3090 Ti".to_owned(),
            vram_mb: Some(24_576),
        }],
        comfyui_found: false,
    }
}

/// The default engine is the one the recipe lists first, whether or not a card is there, and it is
/// never called slow; Burn stays an option that uses the card.
#[test]
fn the_default_engine_is_suggested_with_or_without_a_card_and_is_not_called_slow() {
    let dir = tempfile::tempdir().unwrap();
    let (client, _) = world(&dir);
    let feature = select_subject();
    for profile in [
        with_card(),
        SystemProfile {
            gpus: Vec::new(),
            ..with_card()
        },
    ] {
        let advice = client.engine_advice(&feature, &profile);
        let rten = advice.iter().find(|a| a.provider == "rten").unwrap();
        assert!(rten.recommended, "{advice:?}");
        assert_eq!(rten.pace, Some(Pace::Processor));
        assert!(!rten.reason.contains("slow"), "{}", rten.reason);
        assert_eq!(advice.iter().filter(|a| a.recommended).count(), 1);
        let burn = advice.iter().find(|a| a.provider == "burn").unwrap();
        assert!(burn.usable && !burn.recommended);
    }
}
