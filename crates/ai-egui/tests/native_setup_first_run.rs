//! The setup widget on the first run: no tick box unless a licence needs one, and a way to take
//! the suggested folder when none is chosen. Real layout and clicks; tiny files, no network.
#[path = "../../ai-client/tests/native_setup_support/mod.rs"]
mod fixtures;
use craft_ai_client::{AiClient, FolderDesk, ProviderFactory, SetupSession, SetupTask};
use craft_ai_core::{AiError, AppId, model_folders::FolderOrigin};
use craft_ai_egui::{
    ACCEPT_LICENCES, DOWNLOAD_MODEL, NativeSetupAction, USE_SUGGESTED_FOLDER, native_model_setup,
};
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

/// A harness around a setup session. `licence` says whether the files' licences need agreement
/// and `folder` whether a default model folder is registered.
fn harness(licence: bool, folder: bool) -> Harness<'static, State> {
    let (dir, desk, _) = fixtures::world();
    let desk = if folder {
        desk
    } else {
        FolderDesk::new(SharedStore::new(dir.path().join("empty.json")))
    };
    let store = if folder {
        "connections.json"
    } else {
        "empty.json"
    };
    let mut factory = ProviderFactory::standard();
    factory.register("candle", |_, _| {
        Err(AiError::unsupported("setup does not run inference"))
    });
    let client = AiClient::new(
        AppId::new("test-app").unwrap(),
        SharedStore::new(dir.path().join(store)),
        Arc::new(MemoryStore::new()),
    )
    .with_factory(factory);
    let mut feature = fixtures::feature();
    for model in &mut fixtures::candle_mut(&mut feature).models {
        model.licence.accept_required = licence;
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

/// Files with no licence to agree to show no tick box, and Download works straight away.
#[test]
fn files_with_no_licence_to_accept_show_no_tick_box() {
    let mut h = harness(false, true);
    assert!(h.query_by_label(ACCEPT_LICENCES).is_none());
    assert!(
        !h.get_by_label(DOWNLOAD_MODEL)
            .accesskit_node()
            .is_disabled()
    );
    h.get_by_label(DOWNLOAD_MODEL).click();
    h.run();
    assert!(matches!(h.state().last, Some(NativeSetupAction::Work(_))));
}

/// Files whose licence needs agreement still show the box and still wait for it.
#[test]
fn files_that_need_agreement_still_wait_for_the_tick() {
    let h = harness(true, true);
    h.get_by_label(ACCEPT_LICENCES);
    assert!(
        h.get_by_label(DOWNLOAD_MODEL)
            .accesskit_node()
            .is_disabled()
    );
}

/// With no model folder the widget offers the suggested one, keeps Download off, and reports the
/// choice for the host to act on.
#[test]
fn with_no_folder_the_suggested_one_is_offered() {
    let mut h = harness(false, false);
    assert!(
        h.get_by_label(DOWNLOAD_MODEL)
            .accesskit_node()
            .is_disabled()
    );
    if craft_ai_native::paths::suggested_models_dir().is_none() {
        assert!(h.query_by_label(USE_SUGGESTED_FOLDER).is_none());
        return;
    }
    h.get_by_label(USE_SUGGESTED_FOLDER).click();
    h.run();
    assert!(matches!(
        h.state().last,
        Some(NativeSetupAction::UseSuggestedFolder)
    ));
    assert!(!h.state().session.busy());
}

/// With a folder chosen the suggestion is not offered.
#[test]
fn with_a_folder_chosen_the_suggestion_is_not_offered() {
    let h = harness(false, true);
    assert!(h.query_by_label(USE_SUGGESTED_FOLDER).is_none());
}

/// The suggested folder is named before the person is asked to take it.
#[test]
fn the_suggested_folder_is_shown_before_it_can_be_taken() {
    let Some(suggested) = craft_ai_native::paths::suggested_models_dir() else {
        return;
    };
    let h = harness(false, false);
    h.get_by_label(&format!(
        "Suggested folder: {}",
        suggested.to_string_lossy()
    ));
    h.get_by_label(USE_SUGGESTED_FOLDER);
}

/// A worker checking discovered files can have no chosen download folder yet. The suggested
/// folder action is visible but cannot change the plan while that worker owns the session.
#[test]
fn a_busy_session_cannot_take_the_suggested_folder() {
    let (dir, desk, models) = fixtures::world();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let mut factory = ProviderFactory::standard();
    factory.register("candle", |_, _| Err(AiError::unsupported("not inference")));
    let client = AiClient::new(
        AppId::new("test-app").unwrap(),
        shared.clone(),
        Arc::new(MemoryStore::new()),
    )
    .with_factory(factory);
    let feature = fixtures::feature();
    let route = feature
        .engine_recipe("candle")
        .unwrap()
        .native
        .iter()
        .find(|r| r.provider == "candle")
        .unwrap();
    let directory = models.join(&route.models[0].folder);
    std::fs::create_dir_all(&directory).unwrap();
    for (index, file) in route.models.iter().enumerate() {
        std::fs::write(directory.join(&file.filename), vec![b'x'; index + 1]).unwrap();
    }
    shared
        .update(|store| {
            store.model_folders[0].origin = FolderOrigin::ComfyUi;
            store.model_folders[0].default = false;
        })
        .unwrap();
    let plan = desk
        .prepare_native_setup(&feature, "candle", &client)
        .unwrap();
    assert!(plan.destination().is_none());
    assert!(plan.existing_directory().is_some());
    let mut session = SetupSession::new(plan);
    assert_eq!(session.start().unwrap().task(), SetupTask::CheckExisting);
    let mut h = Harness::new_ui_state(
        |ui, state: &mut State| {
            if let Some(action) = native_model_setup(ui, &mut state.session) {
                state.last = Some(action);
            }
        },
        State {
            _dir: dir,
            session,
            last: None,
        },
    );
    h.run();
    if craft_ai_native::paths::suggested_models_dir().is_none() {
        assert!(h.query_by_label(USE_SUGGESTED_FOLDER).is_none());
    } else {
        assert!(
            h.get_by_label(USE_SUGGESTED_FOLDER)
                .accesskit_node()
                .is_disabled()
        );
    }
    assert!(h.state().session.busy());
    assert!(h.state().last.is_none());
}
