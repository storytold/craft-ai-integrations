//! A failed file check must leave the damaged copy alone and require a newly approved folder.
mod native_setup_support;
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;
use craft_ai_client::{AiClient, ProviderFactory, SetupBlock, SetupSession, SetupTask};
use craft_ai_core::{AiError, AppId};
use craft_ai_native::{secrets::MemoryStore, shared::SharedStore};
use native_setup_support::{candle_mut, feature, world};
use std::{path::Path, sync::Arc};

/// A client with a fake adapter, never a model or hosted connection.
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

/// A check failure blocks downloads to the damaged directory. Only a new chosen destination
/// unblocks them; an unrelated plan refresh cannot erase the failed directory from the session.
#[test]
fn a_bad_same_size_file_requires_a_new_folder_and_is_never_overwritten() {
    let (dir, desk, _) = world();
    let mut f = feature();
    for file in &mut candle_mut(&mut f).models {
        file.licence.accept_required = false;
    }
    let original = desk
        .prepare_native_setup(&f, "candle", &app(dir.path()))
        .unwrap();
    let old = original.destination().unwrap().to_owned();
    std::fs::create_dir_all(&old).unwrap();
    for (i, file) in original.route().models.iter().enumerate() {
        std::fs::write(old.join(&file.filename), vec![b'y'; i + 1]).unwrap();
    }
    let bad = old.join(&original.route().models[0].filename);
    let old_bytes = std::fs::read(&bad).unwrap();
    let mut session = SetupSession::new(
        desk.prepare_native_setup(&f, "candle", &app(dir.path()))
            .unwrap(),
    );
    let check = session.start().unwrap();
    assert_eq!(check.task(), SetupTask::CheckExisting);
    let error = check
        .run(
            &craft_ai_comfy_env::Downloader::new(Default::default()),
            None,
            &mut |_, _| {},
        )
        .unwrap_err();
    assert!(session.complete(check.ticket(), Err(error)));
    assert_eq!(session.blocker(), Some(SetupBlock::NeedsFreshFolder));
    assert!(session.start().is_none());
    assert_eq!(
        session.review(
            desk.prepare_native_setup(&f, "candle", &app(dir.path()))
                .unwrap()
        ),
        None
    );
    assert_eq!(session.blocker(), Some(SetupBlock::NeedsFreshFolder));
    let new_root = dir.path().join("new-models");
    desk.add_new(&new_root, None).unwrap();
    let folder = desk
        .list()
        .unwrap()
        .into_iter()
        .find(|folder| Path::new(&folder.path) == new_root)
        .unwrap();
    desk.set_default(&folder.id).unwrap();
    let fresh = desk
        .prepare_native_setup(&f, "candle", &app(dir.path()))
        .unwrap();
    assert_ne!(fresh.destination(), Some(old.as_path()));
    assert_eq!(session.review(fresh), None);
    assert_eq!(session.next_task(), SetupTask::Download);
    assert_eq!(session.blocker(), None);
    let download = session.start().unwrap();
    assert_eq!(download.task(), SetupTask::Download);
    assert_ne!(download.plan().destination(), Some(old.as_path()));
    assert_eq!(std::fs::read(&bad).unwrap(), old_bytes);
}

/// A loopback transfer into a newly chosen folder succeeds without changing the rejected file.
/// The checked result points into the new folder, not into the damaged copy.
#[test]
fn a_fresh_download_leaves_the_rejected_copy_and_checks_the_new_one() {
    use craft_ai_comfy_env::{DownloadConfig, Downloader};
    use craft_ai_features::Source;
    let server = support::Server::start(|request| {
        let index: usize = request.path.rsplit('/').next().unwrap().parse().unwrap();
        support::http_bytes(200, &[], &vec![b'x'; index + 1])
    });
    let (dir, desk, _) = world();
    let mut f = feature();
    for (index, file) in candle_mut(&mut f).models.iter_mut().enumerate() {
        file.licence.accept_required = false;
        file.source = Source::Url {
            url: format!("{}/{}", server.url.trim_end_matches("/v1"), index),
        };
    }
    let original = desk
        .prepare_native_setup(&f, "candle", &app(dir.path()))
        .unwrap();
    let old = original.destination().unwrap().to_owned();
    std::fs::create_dir_all(&old).unwrap();
    for (index, file) in original.route().models.iter().enumerate() {
        std::fs::write(old.join(&file.filename), vec![b'y'; index + 1]).unwrap();
    }
    let mut session = SetupSession::new(
        desk.prepare_native_setup(&f, "candle", &app(dir.path()))
            .unwrap(),
    );
    let check = session.start().unwrap();
    let error = check
        .run(&Downloader::new(Default::default()), None, &mut |_, _| {})
        .unwrap_err();
    assert!(session.complete(check.ticket(), Err(error)));
    let new_root = dir.path().join("approved-new-folder");
    let folder = desk.add_new(&new_root, None).unwrap();
    desk.set_default(&folder.folder.id).unwrap();
    session.review(
        desk.prepare_native_setup(&f, "candle", &app(dir.path()))
            .unwrap(),
    );
    let download = session.start().unwrap();
    let destination = download.plan().destination().unwrap().to_owned();
    assert!(!destination.exists());
    let result = download.run(
        &Downloader::new(DownloadConfig {
            allow_loopback: true,
            allowed_hosts: Some(vec!["127.0.0.1".to_owned()]),
            ..DownloadConfig::default()
        }),
        None,
        &mut |_, _| {},
    );
    assert!(session.complete(download.ticket(), result));
    assert_eq!(server.count(), 3);
    assert_eq!(
        session.checked().unwrap().weights().parent(),
        Some(destination.as_path())
    );
    for (index, file) in f.engine_recipe("candle").unwrap().native[0]
        .models
        .iter()
        .enumerate()
    {
        assert_eq!(
            std::fs::read(old.join(&file.filename)).unwrap(),
            vec![b'y'; index + 1]
        );
        assert_eq!(
            std::fs::read(destination.join(&file.filename)).unwrap(),
            vec![b'x'; index + 1]
        );
    }
}

/// A folder that appears after approval but before the worker starts cannot be overwritten.
#[test]
fn an_intervening_file_in_the_approved_folder_is_left_alone() {
    let (dir, desk, _) = world();
    let mut f = feature();
    for file in &mut candle_mut(&mut f).models {
        file.licence.accept_required = false;
    }
    let original = desk
        .prepare_native_setup(&f, "candle", &app(dir.path()))
        .unwrap();
    let old = original.destination().unwrap().to_owned();
    std::fs::create_dir_all(&old).unwrap();
    for (index, file) in original.route().models.iter().enumerate() {
        std::fs::write(old.join(&file.filename), vec![b'y'; index + 1]).unwrap();
    }
    let mut session = SetupSession::new(
        desk.prepare_native_setup(&f, "candle", &app(dir.path()))
            .unwrap(),
    );
    let check = session.start().unwrap();
    assert!(session.complete(check.ticket(), Err(AiError::invalid("bad hash"))));
    let new_root = dir.path().join("new-folder");
    let folder = desk.add_new(&new_root, None).unwrap();
    desk.set_default(&folder.folder.id).unwrap();
    session.review(
        desk.prepare_native_setup(&f, "candle", &app(dir.path()))
            .unwrap(),
    );
    let download = session.start().unwrap();
    let destination = download.plan().destination().unwrap();
    std::fs::create_dir_all(destination).unwrap();
    let other = destination.join(&f.engine_recipe("candle").unwrap().native[0].models[0].filename);
    std::fs::write(&other, b"other person's file").unwrap();
    let error = download
        .run(
            &craft_ai_comfy_env::Downloader::new(Default::default()),
            None,
            &mut |_, _| {},
        )
        .unwrap_err();
    assert_eq!(error.kind, craft_ai_core::ErrorKind::InvalidRequest);
    assert_eq!(std::fs::read(other).unwrap(), b"other person's file");
}

/// An already selected default that differs from the bad copy was not newly approved after the
/// failed check. It cannot silently become the recovery destination.
#[test]
fn a_preselected_other_folder_is_not_a_new_approval() {
    let (dir, desk, _) = world();
    let mut f = feature();
    for file in &mut candle_mut(&mut f).models {
        file.licence.accept_required = false;
    }
    let old_plan = desk
        .prepare_native_setup(&f, "candle", &app(dir.path()))
        .unwrap();
    let old = old_plan.destination().unwrap();
    std::fs::create_dir_all(old).unwrap();
    for (i, file) in old_plan.route().models.iter().enumerate() {
        std::fs::write(old.join(&file.filename), vec![b'y'; i + 1]).unwrap();
    }
    let preselected = desk
        .add_new(&dir.path().join("already-selected"), None)
        .unwrap();
    desk.set_default(&preselected.folder.id).unwrap();
    let plan = desk
        .prepare_native_setup(&f, "candle", &app(dir.path()))
        .unwrap();
    assert_eq!(plan.existing_directory(), Some(old));
    assert_ne!(plan.destination(), Some(old));
    let mut session = SetupSession::new(plan);
    let check = session.start().unwrap();
    assert!(session.complete(check.ticket(), Err(AiError::invalid("bad hash"))));
    assert_eq!(session.blocker(), Some(SetupBlock::NeedsFreshFolder));
    let newly_approved = desk
        .add_new(&dir.path().join("newly-approved"), None)
        .unwrap();
    desk.set_default(&newly_approved.folder.id).unwrap();
    session.review(
        desk.prepare_native_setup(&f, "candle", &app(dir.path()))
            .unwrap(),
    );
    assert_eq!(session.blocker(), None);
}

/// If the fresh download fails, the damaged copy is still rejected. The next attempt must not
/// check it again or silently resume in the now nonempty new folder.
#[test]
fn a_failed_fresh_download_keeps_the_old_copy_rejected() {
    let (dir, desk, _) = world();
    let mut f = feature();
    for file in &mut candle_mut(&mut f).models {
        file.licence.accept_required = false;
    }
    let original = desk
        .prepare_native_setup(&f, "candle", &app(dir.path()))
        .unwrap();
    let old = original.destination().unwrap().to_owned();
    std::fs::create_dir_all(&old).unwrap();
    for (index, file) in original.route().models.iter().enumerate() {
        std::fs::write(old.join(&file.filename), vec![b'y'; index + 1]).unwrap();
    }
    let mut session = SetupSession::new(
        desk.prepare_native_setup(&f, "candle", &app(dir.path()))
            .unwrap(),
    );
    let check = session.start().unwrap();
    assert!(session.complete(check.ticket(), Err(AiError::invalid("bad hash"))));
    let next = desk
        .add_new(&dir.path().join("first-recovery"), None)
        .unwrap();
    desk.set_default(&next.folder.id).unwrap();
    session.review(
        desk.prepare_native_setup(&f, "candle", &app(dir.path()))
            .unwrap(),
    );
    let download = session.start().unwrap();
    let fresh_path = download.plan().destination().unwrap().to_owned();
    std::fs::create_dir_all(&fresh_path).unwrap();
    assert!(session.complete(
        download.ticket(),
        Err(AiError::new(
            craft_ai_core::ErrorKind::Network,
            "transfer stopped"
        ))
    ));
    assert_eq!(session.next_task(), SetupTask::Download);
    assert_eq!(session.blocker(), Some(SetupBlock::NeedsFreshFolder));
    let final_folder = desk
        .add_new(&dir.path().join("second-recovery"), None)
        .unwrap();
    desk.set_default(&final_folder.folder.id).unwrap();
    session.review(
        desk.prepare_native_setup(&f, "candle", &app(dir.path()))
            .unwrap(),
    );
    assert_eq!(session.blocker(), None);
    assert_eq!(session.start().unwrap().task(), SetupTask::Download);
}
