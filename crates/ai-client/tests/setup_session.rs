//! The same setup state is used by a host's own rendering and the neutral fallback widget.
mod native_setup_support;
use craft_ai_client::{
    AiClient, FolderDesk, NativeSetupPlan, ProviderFactory, SetupSession, SetupTask,
};
use craft_ai_core::{AiError, AppId};
use craft_ai_native::{secrets::MemoryStore, shared::SharedStore};
use native_setup_support::{candle, candle_mut, feature, sam3_mut, world};
use std::sync::Arc;

/// The client of a temporary store, whose engine factory refuses to build a model.
fn app(dir: &tempfile::TempDir) -> AiClient {
    let mut factory = ProviderFactory::standard();
    factory.register("candle", |_, _| {
        Err(AiError::unsupported("setup must not construct a model"))
    });
    AiClient::new(
        AppId::new("test-app").unwrap(),
        SharedStore::new(dir.path().join("connections.json")),
        Arc::new(MemoryStore::new()),
    )
    .with_factory(factory)
}

/// A prepared setup plan on a temporary store, whose engine factory refuses to build a model.
fn prepared() -> (tempfile::TempDir, FolderDesk, NativeSetupPlan) {
    let (dir, desk, _) = world();
    let plan = desk
        .prepare_native_setup(&feature(), "candle", &app(&dir))
        .unwrap();
    (dir, desk, plan)
}

/// A download starts only after explicit approval, consumes it, and cannot start twice while running.
#[test]
fn download_needs_an_explicit_yes_and_cannot_be_started_twice() {
    let (_dir, _desk, plan) = prepared();
    let mut session = SetupSession::new(plan.clone());
    assert!(session.begin(SetupTask::Download).is_none());
    session.accept_licences(true);
    let request = session
        .begin(SetupTask::Download)
        .expect("the approved download is actionable");
    assert_eq!(request.plan(), &plan);
    assert!(request.approved());
    assert!(session.busy());
    assert!(!session.accepted(), "starting consumes the approval");
    // Even a fresh approval cannot start a second job while one is running.
    session.accept_licences(true);
    assert!(session.begin(SetupTask::Download).is_none());
}

/// Reviewing the same plan keeps the approval; a changed destination discards it.
#[test]
fn changing_the_destination_discards_the_old_approval() {
    let (dir, desk, plan) = prepared();
    let mut session = SetupSession::new(plan.clone());
    session.accept_licences(true);
    let _ = session.review(plan);
    assert!(session.accepted(), "the same frame does not reset a choice");
    let next = dir.path().join("next");
    std::fs::create_dir(&next).unwrap();
    let folder = desk.add(&next, None).unwrap();
    desk.set_default(&folder.folder.id).unwrap();
    let mut factory = ProviderFactory::standard();
    factory.register("candle", |_, _| Err(AiError::unsupported("not inference")));
    let app = AiClient::new(
        AppId::new("app").unwrap(),
        SharedStore::new(dir.path().join("connections.json")),
        Arc::new(MemoryStore::new()),
    )
    .with_factory(factory);
    let _ = session.review(
        desk.prepare_native_setup(&feature(), "candle", &app)
            .unwrap(),
    );
    assert!(!session.accepted());
    assert!(session.begin(SetupTask::Download).is_none());
}

/// Progress is not a new target. A plan for the same destination that needs fewer bytes (a file
/// has landed) keeps the approval, the running request and its ticket, and does not cancel the worker.
#[test]
fn progress_alone_does_not_cancel_a_running_request() {
    let (dir, desk, plan) = prepared();
    let destination = plan.destination().unwrap().to_owned();
    let mut session = SetupSession::new(plan.clone());
    session.accept_licences(true);
    let request = session.begin(SetupTask::Download).unwrap();
    std::fs::create_dir_all(&destination).unwrap();
    let first = &plan.route().models[0];
    std::fs::write(
        destination.join(&first.filename),
        vec![b'x'; first.size_bytes as usize],
    )
    .unwrap();
    let later = desk
        .prepare_native_setup(&feature(), "candle", &app(&dir))
        .unwrap();
    assert!(later.download_bytes() < plan.download_bytes());
    assert_eq!(session.review(later), None, "the worker is left running");
    assert!(session.busy());
    assert!(session.complete(request.ticket(), Err(AiError::invalid("done"))));
}

/// A verified result survives the next frame's plan, which now finds the files and needs no bytes.
#[test]
fn a_checked_result_survives_a_plan_that_found_the_files() {
    let (dir, desk, plan) = prepared();
    let destination = plan.destination().unwrap().to_owned();
    std::fs::create_dir_all(&destination).unwrap();
    for (i, file) in plan.route().models.iter().enumerate() {
        std::fs::write(destination.join(&file.filename), vec![b'x'; i + 1]).unwrap();
    }
    let mut session = SetupSession::new(plan);
    session.accept_licences(true);
    let request = session.begin(SetupTask::Download).unwrap();
    let result = request.run(
        &craft_ai_comfy_env::Downloader::new(Default::default()),
        None,
        &mut |_, _| {},
    );
    assert!(session.complete(request.ticket(), result));
    let later = desk
        .prepare_native_setup(&feature(), "candle", &app(&dir))
        .unwrap();
    assert_eq!(later.download_bytes(), 0);
    assert_eq!(session.review(later), None);
    assert!(session.checked().is_some(), "the verified result is kept");
    assert_eq!(session.problem(), None);
}

/// Complete, correctly sized files for `plan`'s route, in `dir`.
fn write_files(dir: &std::path::Path, plan: &NativeSetupPlan) {
    std::fs::create_dir_all(dir).unwrap();
    for (i, file) in plan.route().models.iter().enumerate() {
        std::fs::write(dir.join(&file.filename), vec![b'x'; i + 1]).unwrap();
    }
}

/// Runs a request on this thread, the way a worker would, with no network.
fn run(request: &craft_ai_client::SetupRequest) -> Result<craft_ai_client::CheckedNative, AiError> {
    request.run(
        &craft_ai_comfy_env::Downloader::new(Default::default()),
        None,
        &mut |_, _| {},
    )
}

/// A verified result belongs to the directory it was checked in. When a later plan for the same
/// target finds complete files in another directory (neither where it was checked nor where a
/// download would go), the old result is dropped, so the widget cannot offer to use it.
#[test]
fn a_checked_result_is_dropped_when_discovery_picks_another_directory() {
    let (dir, desk, plan) = prepared();
    let first = dir.path().join("models").join("first");
    write_files(&first, &plan);
    let plan = desk
        .prepare_native_setup(&feature(), "candle", &app(&dir))
        .unwrap();
    assert_eq!(plan.existing_directory(), Some(first.as_path()));
    let mut session = SetupSession::new(plan.clone());
    let request = session.begin(SetupTask::CheckExisting).unwrap();
    assert!(session.complete(request.ticket(), run(&request)));
    assert!(session.checked().is_some());
    let second = dir.path().join("models").join("second");
    write_files(&second, &plan);
    std::fs::remove_dir_all(&first).unwrap();
    let later = desk
        .prepare_native_setup(&feature(), "candle", &app(&dir))
        .unwrap();
    assert_eq!(later.existing_directory(), Some(second.as_path()));
    assert!(plan.same_target(&later));
    assert_eq!(session.review(later), None);
    assert!(
        session.checked().is_none(),
        "the old directory is not offered"
    );
}

/// A late answer for a directory the review has moved away from cannot bring its result back. The
/// request is still the running one, so its ticket is accepted, but the result is not kept.
#[test]
fn a_late_result_for_a_directory_the_review_moved_from_is_not_kept() {
    let (dir, desk, plan) = prepared();
    let first = dir.path().join("models").join("first");
    write_files(&first, &plan);
    let plan = desk
        .prepare_native_setup(&feature(), "candle", &app(&dir))
        .unwrap();
    let mut session = SetupSession::new(plan.clone());
    let request = session.begin(SetupTask::CheckExisting).unwrap();
    let result = run(&request);
    assert!(result.is_ok());
    let second = dir.path().join("models").join("second");
    write_files(&second, &plan);
    std::fs::remove_dir_all(&first).unwrap();
    let later = desk
        .prepare_native_setup(&feature(), "candle", &app(&dir))
        .unwrap();
    assert_eq!(later.existing_directory(), Some(second.as_path()));
    assert_eq!(session.review(later), None, "the request keeps running");
    assert!(session.complete(request.ticket(), result));
    assert!(
        session.checked().is_none(),
        "the old directory is not offered"
    );
    assert_eq!(session.problem(), None, "a late answer is not an error");
}

/// Complete files elsewhere do not turn a download into the wrong answer: a download goes to its
/// destination, and its verified result there is kept even when discovery points at another copy.
#[test]
fn a_download_result_is_kept_when_complete_files_exist_elsewhere() {
    let (dir, desk, plan) = prepared();
    let destination = plan.destination().unwrap().to_owned();
    write_files(&destination, &plan);
    let other = dir.path().join("models").join("zzz-other-copy");
    write_files(&other, &plan);
    let plan = desk
        .prepare_native_setup(&feature(), "candle", &app(&dir))
        .unwrap();
    assert_ne!(plan.existing_directory(), Some(destination.as_path()));
    assert!(plan.existing_directory().is_some());
    let mut session = SetupSession::new(plan);
    session.accept_licences(true);
    let request = session.begin(SetupTask::Download).unwrap();
    assert!(session.complete(request.ticket(), run(&request)));
    assert!(session.checked().is_some(), "the download's result is kept");
}

/// A plan for other files (another route for the same tool) is a different target: the approval
/// goes and the running worker is cancelled.
#[test]
fn a_different_route_discards_the_approval_and_the_running_request() {
    let (dir, desk, plan) = prepared();
    let mut session = SetupSession::new(plan);
    session.accept_licences(true);
    let request = session.begin(SetupTask::Download).unwrap();
    let mut other = feature();
    candle_mut(&mut other).models[0].filename = "other.safetensors".to_owned();
    let changed = desk
        .prepare_native_setup(&other, "candle", &app(&dir))
        .unwrap();
    assert_eq!(session.review(changed), Some(request.ticket()));
    assert!(!session.busy());
    assert!(!session.accepted());
}

/// A finished download yields checked files but writes no binding; using them is a separate step.
#[test]
fn a_completed_request_makes_checked_files_available_but_does_not_bind_them() {
    let (dir, _desk, plan) = prepared();
    let destination = plan.destination().unwrap();
    std::fs::create_dir_all(destination).unwrap();
    for (i, file) in plan.route().models.iter().enumerate() {
        std::fs::write(destination.join(&file.filename), vec![b'x'; i + 1]).unwrap();
    }
    let mut session = SetupSession::new(plan);
    session.accept_licences(true);
    let request = session.begin(SetupTask::Download).unwrap();
    let result = request.run(
        &craft_ai_comfy_env::Downloader::new(Default::default()),
        None,
        &mut |_, _| {},
    );
    assert!(
        result.is_ok(),
        "the requested files are actually checked: {result:?}"
    );
    assert!(session.complete(request.ticket(), result));
    assert!(session.checked().is_some());
    assert!(!session.busy());
    assert!(
        SharedStore::new(dir.path().join("connections.json"))
            .load()
            .unwrap()
            .defaults
            .is_empty()
    );
}

/// A late answer from a cancelled worker is ignored and cannot replace the newer job.
#[test]
fn a_cancelled_workers_late_answer_cannot_replace_a_newer_job() {
    let (_dir, _desk, plan) = prepared();
    let mut session = SetupSession::new(plan);
    session.accept_licences(true);
    let old = session.begin(SetupTask::Download).unwrap();
    assert_eq!(session.cancel(), Some(old.ticket()));
    session.accept_licences(true);
    let next = session.begin(SetupTask::Download).unwrap();
    assert_ne!(old.ticket(), next.ticket());
    assert!(!session.complete(old.ticket(), Err(AiError::invalid("a stale answer"))));
    assert!(session.busy());
    assert_eq!(session.problem(), None);
}

/// A result for a different reviewed directory is rejected, not accepted as this session's.
#[test]
fn a_result_from_another_reviewed_directory_is_not_accepted() {
    let (_dir, _desk, plan) = prepared();
    let mut session = SetupSession::new(plan);
    session.accept_licences(true);
    let request = session.begin(SetupTask::Download).unwrap();
    let (_other_dir, _other_desk, other) = prepared();
    std::fs::create_dir_all(other.destination().unwrap()).unwrap();
    for (i, file) in other.route().models.iter().enumerate() {
        std::fs::write(
            other.destination().unwrap().join(&file.filename),
            vec![b'x'; i + 1],
        )
        .unwrap();
    }
    let result = other.download(craft_ai_client::NativeInstall {
        downloader: &craft_ai_comfy_env::Downloader::new(Default::default()),
        token: None,
        licences_accepted: true,
        cancel: &std::sync::atomic::AtomicBool::new(false),
        progress: &mut |_, _| {},
    });
    assert!(result.is_ok());
    assert!(session.complete(request.ticket(), result));
    assert!(session.checked().is_none());
    assert_eq!(
        session.problem(),
        Some(craft_ai_core::ErrorKind::InvalidRequest)
    );
}

/// Cancelling stops a worker that has not started, before it creates any file.
#[test]
fn cancelling_a_session_stops_an_unstarted_worker_before_it_creates_files() {
    let (_dir, _desk, plan) = prepared();
    let destination = plan.destination().unwrap().to_owned();
    let mut session = SetupSession::new(plan);
    session.accept_licences(true);
    let request = session.begin(SetupTask::Download).unwrap();
    session.cancel();
    let result = request.run(
        &craft_ai_comfy_env::Downloader::new(Default::default()),
        None,
        &mut |_, _| {},
    );
    assert_eq!(
        result.unwrap_err().kind,
        craft_ai_core::ErrorKind::Cancelled
    );
    assert!(!destination.exists());
}

/// Progress reported with an older request's ticket is refused while a newer request is running.
#[test]
fn progress_from_an_older_request_is_refused_while_a_newer_one_runs() {
    let (_dir, _desk, plan) = prepared();
    let mut session = SetupSession::new(plan);
    session.accept_licences(true);
    let old = session.begin(SetupTask::Download).unwrap();
    session.cancel();
    session.accept_licences(true);
    let current = session.begin(SetupTask::Download).unwrap();
    let progress = craft_ai_client::SetupProgress {
        phase: craft_ai_client::SetupPhase::Verifying,
        done: 1,
        total: 2,
    };
    assert!(!session.report_progress(old.ticket(), 0, progress));
    assert_eq!(session.progress(), None);
    assert!(session.report_progress(current.ticket(), 0, progress));
}

/// A checked result from another runtime is refused even when it names the same weights file.
#[test]
fn a_result_from_another_runtime_is_not_accepted() {
    let (dir, desk, _) = world();
    let mut factory = ProviderFactory::standard();
    for provider in ["candle", "other"] {
        factory.register(provider, |_, _| {
            Err(AiError::unsupported("setup must not construct a model"))
        });
    }
    let app = AiClient::new(
        AppId::new("test-app").unwrap(),
        SharedStore::new(dir.path().join("connections.json")),
        Arc::new(MemoryStore::new()),
    )
    .with_factory(factory);
    let mut feature = feature();
    let mut other = candle(&feature).clone();
    other.provider = "other".into();
    sam3_mut(&mut feature).native.push(other);
    let candle = desk.prepare_native_setup(&feature, "candle", &app).unwrap();
    let elsewhere = desk.prepare_native_setup(&feature, "other", &app).unwrap();
    let destination = candle.destination().unwrap();
    std::fs::create_dir_all(destination).unwrap();
    for (i, file) in candle.route().models.iter().enumerate() {
        std::fs::write(destination.join(&file.filename), vec![b'x'; i + 1]).unwrap();
    }
    let result = elsewhere.download(craft_ai_client::NativeInstall {
        downloader: &craft_ai_comfy_env::Downloader::new(Default::default()),
        token: None,
        licences_accepted: true,
        cancel: &std::sync::atomic::AtomicBool::new(false),
        progress: &mut |_, _| {},
    });
    assert!(result.is_ok());
    let mut session = SetupSession::new(candle);
    session.accept_licences(true);
    let request = session.begin(SetupTask::Download).unwrap();
    assert!(session.complete(request.ticket(), result));
    assert!(session.checked().is_none());
    assert_eq!(
        session.problem(),
        Some(craft_ai_core::ErrorKind::InvalidRequest)
    );
}

/// Progress counts only for the current request and a real file index, and clears on cancel.
#[test]
fn progress_belongs_to_a_current_request_and_a_real_file() {
    let (_dir, _desk, plan) = prepared();
    let mut session = SetupSession::new(plan);
    session.accept_licences(true);
    let request = session.begin(SetupTask::Download).unwrap();
    let progress = craft_ai_client::SetupProgress {
        phase: craft_ai_client::SetupPhase::Verifying,
        done: 1,
        total: 2,
    };
    assert!(session.report_progress(request.ticket(), 0, progress));
    assert_eq!(session.progress(), Some((0, progress)));
    assert!(!session.report_progress(request.ticket(), usize::MAX, progress));
    session.cancel();
    assert_eq!(session.progress(), None);
    assert!(!session.report_progress(request.ticket(), 0, progress));
}
