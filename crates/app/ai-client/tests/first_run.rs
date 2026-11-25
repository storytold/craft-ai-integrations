//! The one-click first run: a suggested folder, a start button that knows what to do, no tick box
//! unless a licence needs one, and adopting only files that passed their checks. Scripted
//! localhost only; no real download, model or service is used.
mod native_setup_support;
use craft_ai_client::{
    AiClient, FolderDesk, NativeSetupPlan, ProviderFactory, SetupBlock, SetupSession, SetupTask,
    ToolDesk,
};
use craft_ai_core::{AiError, AppId, Binding, ConnectionId, ModelId, RoleId};
use craft_ai_features::Feature;
use craft_ai_native::{secrets::MemoryStore, shared::SharedStore};
use native_setup_support::{candle_mut, feature, world};
use std::path::{Path, PathBuf};
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

/// The fixture feature with every file's licence needing no agreement, like the SAM 2.1 files.
fn no_licence() -> Feature {
    let mut f = feature();
    for model in &mut candle_mut(&mut f).models {
        model.licence.accept_required = false;
    }
    f
}

/// A plan for `feature` on the store in `dir`.
fn plan_for(desk: &FolderDesk, feature: &Feature, dir: &Path) -> NativeSetupPlan {
    desk.prepare_native_setup(feature, "candle", &app(dir))
        .unwrap()
}

/// Puts every file of `plan` at its destination, small enough to hash, as the fixtures expect.
fn put_files(plan: &NativeSetupPlan) -> PathBuf {
    let destination = plan.destination().unwrap().to_owned();
    std::fs::create_dir_all(&destination).unwrap();
    for (i, file) in plan.route().models.iter().enumerate() {
        std::fs::write(destination.join(&file.filename), vec![b'x'; i + 1]).unwrap();
    }
    destination
}

/// Files whose licences need no agreement download without a tick; files that do still wait for it.
#[test]
fn a_download_asks_for_agreement_only_when_a_licence_needs_it() {
    let (dir, desk, _) = world();
    let needed = plan_for(&desk, &feature(), dir.path());
    assert!(needed.needs_agreement());
    let mut session = SetupSession::new(needed);
    assert_eq!(session.blocker(), Some(SetupBlock::NeedsAgreement));
    assert!(session.start().is_none(), "no tick, no download");
    session.accept_licences(true);
    assert_eq!(session.blocker(), None);

    let free = plan_for(&desk, &no_licence(), dir.path());
    assert!(!free.needs_agreement());
    let mut session = SetupSession::new(free);
    assert_eq!(session.blocker(), None, "nothing to agree to");
    let request = session.start().expect("starts without a tick");
    assert_eq!(request.task(), SetupTask::Download);
    assert!(!request.approved(), "no approval was given or needed");
}

/// The single start button checks complete files already on disk and downloads only when none are.
#[test]
fn start_checks_files_that_are_there_and_downloads_when_they_are_not() {
    let (dir, desk, _) = world();
    let f = no_licence();
    let empty = plan_for(&desk, &f, dir.path());
    assert!(empty.existing_directory().is_none());
    assert_eq!(
        SetupSession::new(empty.clone()).next_task(),
        SetupTask::Download
    );

    put_files(&empty);
    let found = plan_for(&desk, &f, dir.path());
    assert!(found.existing_directory().is_some());
    let mut session = SetupSession::new(found);
    assert_eq!(session.next_task(), SetupTask::CheckExisting);
    let request = session.start().expect("checking needs no folder or tick");
    assert_eq!(request.task(), SetupTask::CheckExisting);
    assert_eq!(session.blocker(), Some(SetupBlock::Busy));
    assert!(session.start().is_none(), "one request at a time");
}

/// With no default model folder the start button names the folder as the reason, and taking the
/// suggested folder removes it.
#[test]
fn without_a_folder_the_reason_is_the_folder_until_the_suggestion_is_taken() {
    let dir = tempfile::tempdir().unwrap();
    let desk = FolderDesk::new(SharedStore::new(dir.path().join("connections.json")));
    let f = no_licence();
    let mut session = SetupSession::new(plan_for(&desk, &f, dir.path()));
    assert_eq!(session.blocker(), Some(SetupBlock::NeedsFolder));
    assert!(session.start().is_none());

    let suggested = dir.path().join("suggested").join("models");
    desk.ensure_default_at(&suggested).unwrap();
    assert!(session.review(plan_for(&desk, &f, dir.path())).is_none());
    assert_eq!(session.blocker(), None);
    assert!(session.start().is_some());
}

/// A plan the app cannot run is named as such, never started.
#[test]
fn a_plan_the_app_cannot_run_is_blocked_as_unavailable() {
    let (dir, desk, _) = world();
    let bare = AiClient::new(
        AppId::new("bare").unwrap(),
        SharedStore::new(dir.path().join("connections.json")),
        Arc::new(MemoryStore::new()),
    );
    // A plain client has no Candle engine registered, so it cannot run this option.
    let plan = desk
        .prepare_native_setup(&no_licence(), "candle", &bare)
        .unwrap();
    let mut session = SetupSession::new(plan);
    assert_eq!(session.blocker(), Some(SetupBlock::Unavailable));
    assert!(session.start().is_none());
}

/// No folder yet: the suggestion is made and becomes the default. A default the person already
/// has is returned untouched and the suggestion is not created.
#[test]
fn the_suggested_folder_is_made_only_when_there_is_no_default() {
    let dir = tempfile::tempdir().unwrap();
    let desk = FolderDesk::new(SharedStore::new(dir.path().join("connections.json")));
    let suggested = dir.path().join("ArtCraft").join("models");
    let made = desk.ensure_default_at(&suggested).unwrap();
    assert!(made.default && suggested.is_dir());
    assert_eq!(desk.list().unwrap().len(), 1);
    assert_eq!(desk.ensure_default_at(&suggested).unwrap().id, made.id);
    assert_eq!(desk.list().unwrap().len(), 1, "asking again adds nothing");

    let (dir2, desk2, models) = world();
    let other = dir2.path().join("never-made");
    let kept = desk2.ensure_default_at(&other).unwrap();
    assert_eq!(Path::new(&kept.path), models.as_path());
    assert!(
        !other.exists(),
        "a chosen default is never replaced by the suggestion"
    );

    // With two chosen folders the one made the default stays the default.
    let second = dir2.path().join("second");
    std::fs::create_dir(&second).unwrap();
    let added = desk2.add(&second, None).unwrap();
    desk2.set_default(&added.folder.id).unwrap();
    assert_eq!(desk2.ensure_default_at(&other).unwrap().id, added.folder.id);
}

/// Only checked files can be adopted. The first adoption binds the tool; a later one leaves the
/// choice in place and says which choice stayed.
#[test]
fn adopting_checked_files_binds_once_and_reports_the_choice_it_kept() {
    let (dir, desk, _) = world();
    let f = no_licence();
    let plan = plan_for(&desk, &f, dir.path());
    put_files(&plan);
    let mut session = SetupSession::new(plan_for(&desk, &f, dir.path()));
    let request = session.start().unwrap();
    let result = request.run(
        &craft_ai_comfy_env::Downloader::new(Default::default()),
        None,
        &mut |_, _| {},
    );
    assert!(session.complete(request.ticket(), result));
    let checked = session.checked().expect("the files passed their checks");

    let shared = SharedStore::new(dir.path().join("connections.json"));
    let tools = ToolDesk::new(app(dir.path()), shared.clone(), None);
    let first = tools.adopt(checked).unwrap();
    assert!(first.bound && first.kept.is_none());
    let defaults = shared.load().unwrap().defaults;
    assert_eq!(defaults.len(), 1);
    assert_eq!(defaults[0].connection, first.connection);

    // Someone chose another model for the tool in the meantime: adopting changes nothing.
    let other = Binding {
        role: RoleId::new("select-subject").unwrap(),
        connection: ConnectionId::new("elsewhere").unwrap(),
        model: ModelId::new("other").unwrap(),
    };
    shared
        .update(|store| {
            store.defaults.clear();
            store.defaults.push(other.clone());
        })
        .unwrap();
    let second = tools.adopt(checked).unwrap();
    assert!(!second.bound);
    assert_eq!(second.kept, Some((other.connection, other.model)));
    assert_eq!(shared.load().unwrap().defaults.len(), 1);
}
