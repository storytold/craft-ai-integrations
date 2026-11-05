//! Downloads through the real native installer against scripted localhost responses only.
mod native_setup_support;
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_client::NativeInstall;
use craft_ai_comfy_env::{DownloadConfig, Downloader};
use craft_ai_features::{Feature, Source, State, plan_native};
use native_setup_support::{candle, candle_mut, feature, world};
use std::sync::atomic::AtomicBool;
use support::{Reply, Server};

/// A raw HTTP 200 reply carrying `body`.
fn response(body: &[u8]) -> Reply {
    let mut bytes = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(body);
    Reply::Raw(bytes)
}

/// A scripted source for the three native files; `corrupt` makes `vocab.json` the wrong size.
fn serving(corrupt: bool) -> Server {
    Server::start(move |request| match request.path.as_str() {
        "/v1/model.safetensors" => response(b"x"),
        "/v1/vocab.json" if corrupt => response(b"yy"),
        "/v1/vocab.json" => response(b"xx"),
        "/v1/merges.txt" => response(b"xxx"),
        _ => Reply::Json(404, "{}".into()),
    })
}

/// The test feature with every native file pointed at the scripted server.
fn from_server(server: &Server) -> Feature {
    let mut feature = feature();
    for model in &mut candle_mut(&mut feature).models {
        model.source = Source::Url {
            url: format!("{}/{}", server.url, model.filename),
        };
    }
    feature
}

/// A downloader that may reach the loopback test server.
fn downloader() -> Downloader {
    Downloader::new(DownloadConfig {
        allow_loopback: true,
        ..DownloadConfig::default()
    })
}

/// Without licence approval the install fails before any request is made and nothing is created.
#[test]
fn declining_a_licence_never_contacts_even_the_scripted_source() {
    let server = serving(false);
    let feature = from_server(&server);
    let (_dir, desk, _) = world();
    let destination = desk.native_download_dir(&feature, "candle").unwrap();
    assert!(
        desk.install_native(
            &feature,
            "candle",
            NativeInstall {
                downloader: &downloader(),
                token: None,
                licences_accepted: false,
                cancel: &AtomicBool::new(false),
                progress: &mut |_, _| {},
            }
        )
        .is_err()
    );
    assert_eq!(server.count(), 0);
    assert!(!destination.exists());
}

/// Every declared file downloads and verifies, and a second install reuses them without another request.
#[test]
fn all_declared_files_download_verify_and_can_be_reused_without_another_request() {
    let server = serving(false);
    let feature = from_server(&server);
    let (_dir, desk, _) = world();
    let downloader = downloader();
    let cancel = AtomicBool::new(false);
    let mut progress_seen = Vec::new();
    let directory = desk
        .install_native(
            &feature,
            "candle",
            NativeInstall {
                downloader: &downloader,
                token: None,
                licences_accepted: true,
                cancel: &cancel,
                progress: &mut |index, progress| progress_seen.push((index, progress)),
            },
        )
        .unwrap();
    assert_eq!(server.count(), 3);
    for (index, model) in candle(&feature).models.iter().enumerate() {
        assert_eq!(
            std::fs::read(directory.join(&model.filename)).unwrap(),
            vec![b'x'; model.size_bytes as usize]
        );
        assert!(
            progress_seen
                .iter()
                .any(|(i, p)| *i == index && p.phase == craft_ai_comfy_env::Phase::Verifying)
        );
    }
    let weights = desk
        .verify_native_files(&feature, "candle", &cancel, &mut |_, _| {})
        .unwrap();
    assert_eq!(weights, directory.join("model.safetensors"));
    assert_eq!(
        desk.install_native(
            &feature,
            "candle",
            NativeInstall {
                downloader: &downloader,
                token: None,
                licences_accepted: true,
                cancel: &cancel,
                progress: &mut |_, _| {},
            }
        )
        .unwrap(),
        directory
    );
    assert_eq!(
        server.count(),
        3,
        "reusing verified files needs no network request"
    );
}

/// When one file fails its check, the partial install is reported as an error and is not found as a ready model.
#[test]
fn a_failed_file_never_turns_a_partial_install_into_a_ready_model() {
    let server = serving(true);
    let feature = from_server(&server);
    let (_dir, desk, _) = world();
    let destination = desk.native_download_dir(&feature, "candle").unwrap();
    let error = desk
        .install_native(
            &feature,
            "candle",
            NativeInstall {
                downloader: &downloader(),
                token: None,
                licences_accepted: true,
                cancel: &AtomicBool::new(false),
                progress: &mut |_, _| {},
            },
        )
        .unwrap_err();
    assert_eq!(error.kind, craft_ai_core::ErrorKind::Provider);
    assert_eq!(server.count(), 2);
    assert!(destination.join("model.safetensors").is_file());
    assert!(!destination.join("vocab.json").exists());
    let found = desk.native_files(&feature, "candle").unwrap();
    assert_eq!(
        plan_native(&feature, "candle", &found.inventory).state,
        State::NeedsInstall
    );
    assert!(
        desk.verify_native_files(&feature, "candle", &AtomicBool::new(false), &mut |_, _| {})
            .is_err()
    );
}

/// One downloaded and checked choice is used by two apps through the shared store, with no second setup.
#[test]
fn a_downloaded_and_checked_choice_runs_in_two_apps_without_repeating_setup() {
    use craft_ai_client::{AiClient, EngineRegistry, ProviderFactory, RoleStatus, ToolDesk};
    use craft_ai_core::{AppId, ConsentLedger, Event, InputRole, JobState, Payload, Request};
    use craft_ai_engine::testing::FakeRunner;
    use craft_ai_native::secrets::MemoryStore;
    use craft_ai_native::shared::SharedStore;
    use std::sync::Arc;
    use std::time::Duration;

    let server = serving(false);
    let feature = from_server(&server);
    let (dir, folders, _) = world();
    let cancel = AtomicBool::new(false);
    folders
        .install_native(
            &feature,
            "candle",
            NativeInstall {
                downloader: &downloader(),
                token: None,
                licences_accepted: true,
                cancel: &cancel,
                progress: &mut |_, _| {},
            },
        )
        .unwrap();
    let weights = folders
        .verify_native_files(&feature, "candle", &cancel, &mut |_, _| {})
        .unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let app = |id: &str| {
        let mut registry = EngineRegistry::new();
        registry.register(FakeRunner::new("candle", "sam3", "image.segment"));
        let mut factory = ProviderFactory::standard();
        factory.register_engines(registry);
        AiClient::new(
            AppId::new(id).unwrap(),
            shared.clone(),
            Arc::new(MemoryStore::new()),
        )
        .with_factory(factory)
    };
    ToolDesk::new(app("launcher"), shared.clone(), None)
        .enable_native_with(&feature, "candle", &weights)
        .unwrap();
    let role = feature.role().unwrap();
    for id in ["first-app", "second-app"] {
        let client = app(id);
        assert!(matches!(client.status(&role), RoleStatus::Ready(_)));
        let request = Request::new(role.capability.clone()).with_input(
            InputRole::Source,
            Payload::text("scripted input, not a real image"),
        );
        let ledger = ConsentLedger::default();
        let plan = client.plan(&role, request).unwrap();
        assert!(!plan.needs_consent(&ledger));
        let done = client
            .start(&plan, &ledger)
            .unwrap()
            .wait(Duration::from_millis(2), Duration::from_secs(10))
            .unwrap();
        assert_eq!(done.state, JobState::Succeeded);
        assert!(done.events.iter().any(|event| matches!(event,
            Event::Output(output) if output.payload == Payload::text("done"))));
    }
    assert_eq!(server.count(), 3);
    assert!(
        shared.load().unwrap().apps.is_empty(),
        "neither app needs its own binding"
    );
}
