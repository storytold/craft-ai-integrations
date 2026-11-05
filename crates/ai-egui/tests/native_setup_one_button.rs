//! The setup widget shows one start button, and the session decides what it does: check files
//! already on disk, or download when there are none or they failed their check. Real layout and
//! clicks; tiny files, no network.
#[path = "../../ai-client/tests/native_setup_support/mod.rs"]
mod fixtures;
use craft_ai_client::{AiClient, ProviderFactory, SetupSession, SetupTask};
use craft_ai_core::{AiError, AppId};
use craft_ai_egui::{CHECK_FILES, DOWNLOAD_MODEL, NativeSetupAction, native_model_setup};
use craft_ai_native::{secrets::MemoryStore, shared::SharedStore};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use std::sync::Arc;

struct State {
    _dir: tempfile::TempDir,
    session: SetupSession,
    last: Option<NativeSetupAction>,
}

/// A harness around a setup session whose model folder already holds every file, with no
/// licence to agree to.
fn harness_with_files() -> Harness<'static, State> {
    let (dir, desk, _) = fixtures::world();
    let mut factory = ProviderFactory::standard();
    factory.register("candle", |_, _| {
        Err(AiError::unsupported("setup does not run inference"))
    });
    let client = AiClient::new(
        AppId::new("test-app").unwrap(),
        SharedStore::new(dir.path().join("connections.json")),
        Arc::new(MemoryStore::new()),
    )
    .with_factory(factory);
    let mut feature = fixtures::feature();
    for model in &mut fixtures::candle_mut(&mut feature).models {
        model.licence.accept_required = false;
    }
    let plan = desk
        .prepare_native_setup(&feature, "candle", &client)
        .unwrap();
    let destination = plan.destination().unwrap().to_owned();
    std::fs::create_dir_all(&destination).unwrap();
    for (i, file) in plan.route().models.iter().enumerate() {
        std::fs::write(destination.join(&file.filename), vec![b'x'; i + 1]).unwrap();
    }
    let plan = desk
        .prepare_native_setup(&feature, "candle", &client)
        .unwrap();
    let mut h = Harness::new_ui_state(
        |ui, state: &mut State| {
            if let Some(action) = native_model_setup(ui, &mut state.session) {
                state.last = Some(action);
            }
        },
        State {
            _dir: dir,
            session: SetupSession::new(plan),
            last: None,
        },
    );
    h.run();
    h
}

/// With complete files on disk the only start button is the check, and it asks the session for
/// the check, not a download.
#[test]
fn with_files_on_disk_the_one_button_checks_them() {
    let mut h = harness_with_files();
    assert!(h.query_by_label(DOWNLOAD_MODEL).is_none());
    assert!(!h.get_by_label(CHECK_FILES).accesskit_node().is_disabled());
    h.get_by_label(CHECK_FILES).click();
    h.run();
    match &h.state().last {
        Some(NativeSetupAction::Work(request)) => {
            assert_eq!(request.task(), SetupTask::CheckExisting)
        }
        other => panic!("expected a check, got {other:?}"),
    }
    assert!(h.state().session.busy());
}

/// A failed check blocks downloading to the old folder, and the screen asks for a new one.
#[test]
fn after_a_failed_check_the_one_button_downloads() {
    let mut h = harness_with_files();
    h.get_by_label(CHECK_FILES).click();
    h.run();
    let Some(NativeSetupAction::Work(request)) = h.state().last.clone() else {
        panic!("expected a check to start");
    };
    h.state_mut().last = None;
    assert!(
        h.state_mut()
            .session
            .complete(request.ticket(), Err(AiError::invalid("no match")))
    );
    h.run();
    assert!(h.query_by_label(CHECK_FILES).is_none());
    assert!(
        h.get_by_label(DOWNLOAD_MODEL)
            .accesskit_node()
            .is_disabled()
    );
    assert!(h.query_by_label("These files did not pass their check. Choose a new empty model folder for the download. The old files will be left alone.").is_some());
    assert!(!h.state().session.busy());
}
