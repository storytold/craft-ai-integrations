//! The shared setup widget through actual layout and clicks, with tiny checked local files.
#[path = "../../ai-client/tests/native_setup_support/mod.rs"]
mod fixtures;
use craft_ai_client::{AiClient, ProviderFactory, SetupSession};
use craft_ai_core::{AiError, AppId};
use craft_ai_egui::{ACCEPT_LICENCES, DOWNLOAD_MODEL, NativeSetupAction, native_model_setup};
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

/// A kittest harness around one setup session on a temporary store; `available` says whether the app has registered the engine.
fn harness(available: bool) -> Harness<'static, State> {
    let (dir, desk, _) = fixtures::world();
    let mut factory = ProviderFactory::standard();
    factory.unregister("candle");
    if available {
        factory.register("candle", |_, _| {
            Err(AiError::unsupported("setup does not run inference"))
        });
    }
    let client = AiClient::new(
        AppId::new("test-app").unwrap(),
        SharedStore::new(dir.path().join("connections.json")),
        Arc::new(MemoryStore::new()),
    )
    .with_factory(factory);
    let plan = desk
        .prepare_native_setup(&fixtures::feature(), "candle", &client)
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

/// The widget shows the destination and licence, and Download stays disabled until the licence box is ticked.
#[test]
fn it_reviews_the_tool_destination_and_licence_before_any_download() {
    let mut h = harness(true);
    h.get_by_label("Set up Select subject");
    let destination = h
        .state()
        .session
        .plan()
        .destination()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    h.get_by_label(&destination);
    h.get_by_label(ACCEPT_LICENCES);
    assert!(
        h.get_by_label(DOWNLOAD_MODEL)
            .accesskit_node()
            .is_disabled()
    );
    assert!(h.state().last.is_none());
    h.get_by_label(ACCEPT_LICENCES).click();
    h.run();
    h.get_by_label(DOWNLOAD_MODEL).click();
    h.run();
    assert!(matches!(h.state().last, Some(NativeSetupAction::Work(_))));
    assert!(h.state().session.busy());
}

/// Cancel reports the running ticket, clears the work, and the worker's late answer is then refused.
#[test]
fn cancel_clears_the_running_work_and_refuses_its_late_answer() {
    let mut h = harness(true);
    h.get_by_label(ACCEPT_LICENCES).click();
    h.run();
    h.get_by_label(DOWNLOAD_MODEL).click();
    h.run();
    let ticket = match h.state().last.as_ref().unwrap() {
        NativeSetupAction::Work(request) => request.ticket(),
        _ => panic!("expected work"),
    };
    h.get_by_label("Cancel").click();
    h.run();
    assert!(matches!(h.state().last, Some(NativeSetupAction::Cancel(id)) if id == ticket));
    assert!(!h.state().session.busy());
    assert!(
        !h.state_mut()
            .session
            .complete(ticket, Err(AiError::invalid("late answer")))
    );
}

/// Checked files are offered for use and not called tested; the widget does not write the shared choice itself.
#[test]
fn checked_files_are_offered_for_use_but_are_not_mislabelled_as_tested_inference() {
    let mut h = harness(true);
    let plan = h.state().session.plan().clone();
    std::fs::create_dir_all(plan.destination().unwrap()).unwrap();
    for (i, file) in plan.route().models.iter().enumerate() {
        std::fs::write(
            plan.destination().unwrap().join(&file.filename),
            vec![b'x'; i + 1],
        )
        .unwrap();
    }
    h.get_by_label(ACCEPT_LICENCES).click();
    h.run();
    h.get_by_label(DOWNLOAD_MODEL).click();
    h.run();
    let request = match h.state().last.as_ref().unwrap() {
        NativeSetupAction::Work(request) => request.clone(),
        _ => panic!("expected work"),
    };
    let result = request.run(
        &craft_ai_client::ModelDownloader::new(Default::default()),
        None,
        &mut |_, _| {},
    );
    assert!(result.is_ok());
    assert!(h.state_mut().session.complete(request.ticket(), result));
    h.run();
    h.get_by_label("Files checked. This model has not been tried yet.");
    assert!(h.query_by_label("Ready").is_none());
    h.get_by_label("Use this model").click();
    h.run();
    assert!(matches!(h.state().last, Some(NativeSetupAction::Use(_))));
    let store = SharedStore::new(h.state()._dir.path().join("connections.json"))
        .load()
        .unwrap();
    assert!(
        store.defaults.is_empty(),
        "the host, not the widget, applies this explicit choice"
    );
}

/// Existing files can be checked with no download approval, and the check names the folder they are in.
#[test]
fn existing_files_can_be_checked_without_agreeing_to_a_download() {
    let mut h = harness(true);
    let plan = h.state().session.plan().clone();
    let root = plan.destination().unwrap().parent().unwrap();
    for (i, file) in plan.route().models.iter().enumerate() {
        std::fs::write(root.join(&file.filename), vec![b'x'; i + 1]).unwrap();
    }
    let shared = SharedStore::new(h.state()._dir.path().join("connections.json"));
    let mut factory = ProviderFactory::standard();
    factory.register("candle", |_, _| Err(AiError::unsupported("no inference")));
    let client = AiClient::new(
        AppId::new("app").unwrap(),
        shared.clone(),
        Arc::new(MemoryStore::new()),
    )
    .with_factory(factory);
    let next = craft_ai_client::FolderDesk::new(shared)
        .prepare_native_setup(plan.feature(), "candle", &client)
        .unwrap();
    let _ = h.state_mut().session.review(next);
    h.run();
    h.get_by_label("Check existing files").click();
    h.run();
    let request = match h.state().last.as_ref().unwrap() {
        NativeSetupAction::Work(request) => request.clone(),
        _ => panic!("expected work"),
    };
    assert_eq!(request.task(), craft_ai_client::SetupTask::CheckExisting);
    assert!(!request.approved());
    let checked = request
        .run(
            &craft_ai_client::ModelDownloader::new(Default::default()),
            None,
            &mut |_, _| {},
        )
        .unwrap();
    assert_eq!(checked.weights().parent(), Some(root));
}

/// Find files, Change folder and Back each report their own action to the host.
#[test]
fn folder_and_back_buttons_report_the_right_host_action() {
    for label in ["Find model files", "Change folder", "Back"] {
        let mut h = harness(true);
        h.get_by_label(label).click();
        h.run();
        assert!(matches!(
            (label, h.state().last.as_ref()),
            ("Find model files", Some(NativeSetupAction::FindFiles))
                | ("Change folder", Some(NativeSetupAction::ChangeFolder))
                | ("Back", Some(NativeSetupAction::Back))
        ));
    }
}

/// Progress from a worker shows which file is being checked, and Cancel stays enabled.
#[test]
fn a_worker_reports_checking_progress_without_hiding_cancel() {
    let mut h = harness(true);
    h.get_by_label(ACCEPT_LICENCES).click();
    h.run();
    h.get_by_label(DOWNLOAD_MODEL).click();
    h.run();
    let ticket = match h.state().last.as_ref().unwrap() {
        NativeSetupAction::Work(request) => request.ticket(),
        _ => panic!("expected work"),
    };
    assert!(h.state_mut().session.report_progress(
        ticket,
        0,
        craft_ai_client::SetupProgress {
            phase: craft_ai_client::SetupPhase::Verifying,
            done: 1,
            total: 2
        }
    ));
    h.run();
    h.get_by_label("Checking file 1 of 3");
    assert!(!h.get_by_label("Cancel").accesskit_node().is_disabled());
}

/// A failed download shows fixed, actionable wording and never repeats the source's own message.
#[test]
fn a_download_error_is_actionable_and_does_not_echo_a_sources_message() {
    let mut h = harness(true);
    h.get_by_label(ACCEPT_LICENCES).click();
    h.run();
    h.get_by_label(DOWNLOAD_MODEL).click();
    h.run();
    let ticket = match h.state().last.as_ref().unwrap() {
        NativeSetupAction::Work(request) => request.ticket(),
        _ => panic!("expected work"),
    };
    assert!(h.state_mut().session.complete(
        ticket,
        Err(AiError::new(
            craft_ai_core::ErrorKind::Auth,
            "deliberately-echoed-test-value"
        ))
    ));
    h.run();
    h.get_by_label("Access to these model files was refused. Check access with their maker.");
    assert!(
        h.query_by_label_contains("deliberately-echoed-test-value")
            .is_none()
    );
    assert!(h.query_by_label("Use this model").is_none());
}

/// A route's one-line advice is shown before the person approves anything.
#[test]
fn the_routes_advice_is_shown_with_the_plan() {
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
    fixtures::candle_mut(&mut feature).note =
        Some("Selects by clicking on the picture, not by a description.".into());
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
            session: craft_ai_client::SetupSession::new(plan),
            last: None,
        },
    );
    h.run();
    h.get_by_label("Selects by clicking on the picture, not by a description.");
    h.get_by_label("The model choice is shared with every app that supports this tool.");
}
