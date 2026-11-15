//! A failed check of the files on disk must not be a dead end: the one start button moves on to
//! downloading.
//! Scripted localhost only; tiny files, no real model or service.
mod native_setup_support;
use craft_ai_client::{
    AiClient, FolderDesk, NativeSetupPlan, ProviderFactory, SetupBlock, SetupSession, SetupTask,
};
use craft_ai_core::{AiError, AppId, ErrorKind};
use craft_ai_features::Feature;
use craft_ai_native::{secrets::MemoryStore, shared::SharedStore};
use native_setup_support::{candle_mut, feature, world};
use std::path::Path;
use std::sync::Arc;

/// A client on the store in `dir` whose engine factory refuses to build a model.
fn app(dir: &Path) -> AiClient {
    let mut factory = ProviderFactory::standard();
    factory.register("candle", |_, _| {
        Err(AiError::unsupported("setup must not construct a model"))
    });
    AiClient::new(
        AppId::new("test-app").unwrap(),
        SharedStore::new(dir.join("connections.json")),
        Arc::new(MemoryStore::new()),
    )
    .with_factory(factory)
}

/// The fixture feature with every file's licence needing no agreement.
fn no_licence() -> Feature {
    let mut f = feature();
    for model in &mut candle_mut(&mut f).models {
        model.licence.accept_required = false;
    }
    f
}

/// A plan for `f` on the store in `dir`, after putting every file at its destination.
fn plan_with_files(desk: &FolderDesk, f: &Feature, dir: &Path) -> NativeSetupPlan {
    let plan = desk.prepare_native_setup(f, "candle", &app(dir)).unwrap();
    let destination = plan.destination().unwrap().to_owned();
    std::fs::create_dir_all(&destination).unwrap();
    for (i, file) in plan.route().models.iter().enumerate() {
        std::fs::write(destination.join(&file.filename), vec![b'x'; i + 1]).unwrap();
    }
    desk.prepare_native_setup(f, "candle", &app(dir)).unwrap()
}

/// Files on disk are checked first. When that check fails, the old destination cannot be
/// downloaded to. A new destination must be approved before starting again.
#[test]
fn after_a_failed_check_the_start_button_downloads() {
    let (dir, desk, _) = world();
    let f = no_licence();
    let mut session = SetupSession::new(plan_with_files(&desk, &f, dir.path()));
    assert_eq!(session.next_task(), SetupTask::CheckExisting);
    let request = session.start().unwrap();
    assert_eq!(request.task(), SetupTask::CheckExisting);
    assert!(session.complete(
        request.ticket(),
        Err(AiError::invalid("the files do not match"))
    ));
    assert_eq!(session.problem(), Some(ErrorKind::InvalidRequest));

    assert_eq!(session.next_task(), SetupTask::Download);
    assert_eq!(session.blocker(), Some(SetupBlock::NeedsFreshFolder));
    assert!(
        session
            .review(plan_with_files(&desk, &f, dir.path()))
            .is_none()
    );
    assert_eq!(session.blocker(), Some(SetupBlock::NeedsFreshFolder));
    assert!(session.start().is_none());
}

/// A failed download is not a failed check: the files on disk are still checked first.
#[test]
fn a_failed_download_does_not_stop_the_check() {
    let (dir, desk, _) = world();
    let f = no_licence();
    let mut session = SetupSession::new(plan_with_files(&desk, &f, dir.path()));
    let request = session.begin(SetupTask::Download).unwrap();
    assert!(session.complete(request.ticket(), Err(AiError::invalid("no route"))));
    assert_eq!(session.next_task(), SetupTask::CheckExisting);
}
