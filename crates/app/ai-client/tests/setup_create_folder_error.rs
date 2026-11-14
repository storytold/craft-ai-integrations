//! A newly approved model folder can disappear or become occupied before a worker starts.
mod native_setup_support;
use craft_ai_client::{AiClient, NativeInstall, ProviderFactory};
use craft_ai_core::{AiError, AppId, ErrorKind};
use craft_ai_native::{secrets::MemoryStore, shared::SharedStore};
use native_setup_support::{feature, world};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

/// Build a test client whose fake adapter never reaches model inference.
fn app(dir: &Path) -> AiClient {
    let mut factory = ProviderFactory::standard();
    factory.register("candle", |_, _| Err(AiError::unsupported("not inference")));
    AiClient::new(
        AppId::new("test-app").unwrap(),
        SharedStore::new(dir.join("connections.json")),
        Arc::new(MemoryStore::new()),
    )
    .with_factory(factory)
}

/// Removing the approved parent is not reported as a collision with somebody's files.
#[test]
fn missing_parent_is_reported_as_missing_not_already_existing() {
    let (dir, desk, _) = world();
    let plan = desk
        .prepare_native_setup(&feature(), "candle", &app(dir.path()))
        .unwrap();
    let parent = plan.destination().unwrap().parent().unwrap();
    std::fs::remove_dir_all(parent).unwrap();
    let downloader = craft_ai_comfy_env::Downloader::new(Default::default());
    let cancel = AtomicBool::new(false);
    let error = plan
        .download_fresh(NativeInstall {
            downloader: &downloader,
            token: None,
            licences_accepted: true,
            cancel: &cancel,
            progress: &mut |_, _| {},
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidRequest);
    assert!(error.message.contains("missing"), "{error}");
    assert!(!error.message.contains("already exists"));
    assert!(!parent.exists());
}

/// An occupied destination still reports a collision and never replaces its file.
#[test]
fn occupied_destination_is_reported_as_existing_without_touching_it() {
    let (dir, desk, _) = world();
    let plan = desk
        .prepare_native_setup(&feature(), "candle", &app(dir.path()))
        .unwrap();
    let destination = plan.destination().unwrap();
    std::fs::create_dir(destination).unwrap();
    let other = destination.join("personal.txt");
    std::fs::write(&other, b"untouched").unwrap();
    let downloader = craft_ai_comfy_env::Downloader::new(Default::default());
    let cancel = AtomicBool::new(false);
    let error = plan
        .download_fresh(NativeInstall {
            downloader: &downloader,
            token: None,
            licences_accepted: true,
            cancel: &cancel,
            progress: &mut |_, _| {},
        })
        .unwrap_err();
    assert!(error.message.contains("already exists"), "{error}");
    assert_eq!(std::fs::read(other).unwrap(), b"untouched");
}
