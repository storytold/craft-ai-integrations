//! A built-in model must not advertise an actionable setup route to an app without its engine.
use craft_ai_client::{AiClient, GroupState, ProviderFactory, SetupLink, ToolDesk};
use craft_ai_core::{AiError, AppId, ConnectionId, ModelId};
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;
use std::sync::Arc;

/// The built-in model offers setup only when this app has registered the engine.
#[test]
fn the_builtin_model_only_offers_setup_when_this_app_has_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let client = AiClient::new(
        AppId::new("test-app").unwrap(),
        SharedStore::new(dir.path().join("connections.json")),
        Arc::new(MemoryStore::new()),
    );
    let features = craft_ai_features::builtin();
    let role = features
        .iter()
        .find(|feature| feature.id == "select-subject")
        .unwrap()
        .role()
        .unwrap();
    let view = client
        .picker(&role, &craft_ai_features::setup_options(&features))
        .unwrap();
    let native = view
        .groups
        .iter()
        .find(|g| g.title.starts_with("SAM 3, built in"))
        .unwrap();
    if cfg!(feature = "candle") {
        assert_eq!(native.state, GroupState::NotSetUp);
        assert!(matches!(&native.setup, Some(SetupLink::New { option })
            if option == "native:select-subject:candle"));
    } else {
        assert_eq!(native.state, GroupState::NeedsAdapter);
        assert!(native.setup.is_none());
    }
}

/// A built-in model whose weights file has been removed keeps a way back in. The saved connection
/// is not proof the files are there: the picker offers the same Set up link as a first install and
/// says the files are missing, and it offers none while the file is still there.
#[test]
fn a_built_in_model_whose_file_is_gone_can_be_set_up_again() {
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let mut factory = ProviderFactory::standard();
    factory.register("candle", |_, _| Err(AiError::unsupported("not run here")));
    let client = AiClient::new(
        AppId::new("test-app").unwrap(),
        shared.clone(),
        Arc::new(MemoryStore::new()),
    )
    .with_factory(factory);
    let features = craft_ai_features::builtin();
    let feature = features
        .iter()
        .find(|feature| feature.id == "select-subject")
        .unwrap();
    let role = feature.role().unwrap();
    let weights = dir.path().join("model.safetensors");
    std::fs::write(&weights, b"weights").unwrap();
    let launcher = AiClient::new(
        AppId::new("launcher").unwrap(),
        shared.clone(),
        Arc::new(MemoryStore::new()),
    );
    ToolDesk::new(launcher, shared, None)
        .enable_native_with(feature, "candle", &weights)
        .unwrap();
    let options = craft_ai_features::setup_options(&features);
    let native = |client: &AiClient| {
        client
            .picker(&role, &options)
            .unwrap()
            .groups
            .into_iter()
            .filter(|g| g.title.starts_with("SAM 3, built in") && g.state == GroupState::NotSetUp)
            .collect::<Vec<_>>()
    };
    assert!(
        native(&client).is_empty(),
        "files present: nothing to set up"
    );
    std::fs::remove_file(&weights).unwrap();
    let groups = native(&client);
    assert_eq!(groups.len(), 1, "files gone: a way back in");
    assert!(matches!(&groups[0].setup, Some(SetupLink::New { option })
        if option == "native:select-subject:candle"));
    assert!(groups[0].note.contains("missing"));
}

/// Another built-in connection for the same runtime and role, from another model, is not this
/// model: the option for this one keeps its Set up link.
#[test]
fn a_built_in_connection_for_another_model_does_not_hide_this_one() {
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let mut factory = ProviderFactory::standard();
    factory.register("candle", |_, _| Err(AiError::unsupported("not run here")));
    let client = AiClient::new(
        AppId::new("test-app").unwrap(),
        shared.clone(),
        Arc::new(MemoryStore::new()),
    )
    .with_factory(factory);
    let features = craft_ai_features::builtin();
    let feature = features
        .iter()
        .find(|feature| feature.id == "select-subject")
        .unwrap();
    let role = feature.role().unwrap();
    let weights = dir.path().join("model.safetensors");
    std::fs::write(&weights, b"weights").unwrap();
    let launcher = AiClient::new(
        AppId::new("launcher").unwrap(),
        shared.clone(),
        Arc::new(MemoryStore::new()),
    );
    ToolDesk::new(launcher, shared.clone(), None)
        .enable_native_with(feature, "candle", &weights)
        .unwrap();
    // The file is there, but the connection now belongs to some other model.
    let mut store = shared.load().unwrap();
    store.connections[0].id = ConnectionId::new("local-another-model-candle").unwrap();
    shared.replace(&store).unwrap();
    let options = craft_ai_features::setup_options(&features);
    let view = client.picker(&role, &options).unwrap();
    assert!(
        view.groups
            .iter()
            .any(|g| g.title.starts_with("SAM 3, built in")
                && g.state == GroupState::NotSetUp
                && matches!(&g.setup, Some(SetupLink::New { option })
                if option == "native:select-subject:candle")),
        "this model still has a way in"
    );
}

/// A saved connection with this id but an older model on it does not stand for the current model:
/// the file is there, yet the option for the model the recipe now names keeps its Set up link.
#[test]
fn an_older_model_on_the_same_connection_does_not_hide_the_current_one() {
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let mut factory = ProviderFactory::standard();
    factory.register("candle", |_, _| Err(AiError::unsupported("not run here")));
    let client = AiClient::new(
        AppId::new("test-app").unwrap(),
        shared.clone(),
        Arc::new(MemoryStore::new()),
    )
    .with_factory(factory);
    let features = craft_ai_features::builtin();
    let feature = features
        .iter()
        .find(|feature| feature.id == "select-subject")
        .unwrap();
    let role = feature.role().unwrap();
    let weights = dir.path().join("model.safetensors");
    std::fs::write(&weights, b"weights").unwrap();
    let launcher = AiClient::new(
        AppId::new("launcher").unwrap(),
        shared.clone(),
        Arc::new(MemoryStore::new()),
    );
    ToolDesk::new(launcher, shared.clone(), None)
        .enable_native_with(feature, "candle", &weights)
        .unwrap();
    let mut store = shared.load().unwrap();
    store.connections[0].models[0].id = ModelId::new("sam3-older").unwrap();
    shared.replace(&store).unwrap();
    let options = craft_ai_features::setup_options(&features);
    let view = client.picker(&role, &options).unwrap();
    assert!(
        view.groups
            .iter()
            .any(|g| g.title.starts_with("SAM 3, built in")
                && g.state == GroupState::NotSetUp
                && g.setup.is_some()),
        "the current model still has a way in"
    );
}
