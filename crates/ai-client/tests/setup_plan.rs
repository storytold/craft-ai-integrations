//! A reviewed setup plan keeps its exact files and destination while the shared folder map changes.
mod native_setup_support;
use craft_ai_client::{AiClient, EngineRegistry, ProviderFactory};
use craft_ai_core::AppId;
use craft_ai_engine::testing::FakeRunner;
use craft_ai_native::{secrets::MemoryStore, shared::SharedStore};
use native_setup_support::{candle, feature, world};
use std::sync::{Arc, atomic::AtomicBool};

/// An app client whose factory has a fake `candle` engine registered.
fn client(path: &std::path::Path) -> AiClient {
    let mut registry = EngineRegistry::new();
    registry.register(FakeRunner::new("candle", "sam3", "image.segment"));
    let mut factory = ProviderFactory::standard();
    factory.register_engines(registry);
    AiClient::new(
        AppId::new("test-app").unwrap(),
        SharedStore::new(path.join("connections.json")),
        Arc::new(MemoryStore::new()),
    )
    .with_factory(factory)
}

/// The download size counts only files already in the reviewed destination, not partial sets elsewhere.
#[test]
fn the_download_size_counts_only_files_in_the_reviewed_destination() {
    let (dir, desk, _) = world();
    let elsewhere = dir.path().join("existing");
    std::fs::create_dir(&elsewhere).unwrap();
    let feature = feature();
    let files = &candle(&feature).models;
    std::fs::write(elsewhere.join(&files[0].filename), b"x").unwrap();
    desk.add(&elsewhere, None).unwrap();
    let plan = desk
        .prepare_native_setup(&feature, "candle", &client(dir.path()))
        .unwrap();
    assert_eq!(
        plan.download_bytes(),
        files.iter().map(|f| f.size_bytes).sum::<u64>()
    );
    assert!(plan.existing_directory().is_none());
}

/// An existing model is hashed on review, and nothing is written to the shared bindings.
#[test]
fn a_reviewed_existing_model_is_hashed_without_writing_a_binding() {
    let (dir, desk, models) = world();
    let feature = feature();
    for (i, file) in candle(&feature).models.iter().enumerate() {
        std::fs::write(models.join(&file.filename), vec![b'x'; i + 1]).unwrap();
    }
    let plan = desk
        .prepare_native_setup(&feature, "candle", &client(dir.path()))
        .unwrap();
    let checked = plan
        .check_existing(&AtomicBool::new(false), &mut |_, _| {})
        .unwrap();
    assert_eq!(checked.weights(), models.join("model.safetensors"));
    assert!(
        SharedStore::new(dir.path().join("connections.json"))
            .load()
            .unwrap()
            .defaults
            .is_empty()
    );
    std::fs::write(models.join("vocab.json"), b"yy").unwrap();
    assert!(
        plan.check_existing(&AtomicBool::new(false), &mut |_, _| {})
            .is_err()
    );
}

/// A prepared plan keeps the destination the person reviewed, even if the default folder changes later.
#[test]
fn a_prepared_download_keeps_the_destination_the_person_reviewed() {
    let (dir, desk, models) = world();
    let feature = feature();
    let destination = models.join(&candle(&feature).models[0].folder);
    std::fs::create_dir(&destination).unwrap();
    for (i, file) in candle(&feature).models.iter().enumerate() {
        std::fs::write(destination.join(&file.filename), vec![b'x'; i + 1]).unwrap();
    }
    let plan = desk
        .prepare_native_setup(&feature, "candle", &client(dir.path()))
        .unwrap();
    let next = dir.path().join("next-default");
    std::fs::create_dir(&next).unwrap();
    let added = desk.add(&next, None).unwrap();
    desk.set_default(&added.folder.id).unwrap();
    let cancel = AtomicBool::new(false);
    let checked = plan
        .download(craft_ai_client::NativeInstall {
            downloader: &craft_ai_comfy_env::Downloader::new(Default::default()),
            token: None,
            licences_accepted: true,
            cancel: &cancel,
            progress: &mut |_, _| {},
        })
        .unwrap();
    assert!(checked.weights().starts_with(&destination));
    assert!(std::fs::read_dir(&next).unwrap().next().is_none());
}
