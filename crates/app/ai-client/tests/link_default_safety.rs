use craft_ai_client::FolderDesk;
use craft_ai_core::model_folders::download_target;
use craft_ai_native::shared::SharedStore;

/// Linking an existing folder is discovery, not approval to write downloads into it.
#[test]
fn linked_folder_never_becomes_a_download_target_without_an_explicit_choice() {
    let temp = tempfile::tempdir().unwrap();
    let linked = temp.path().join("existing");
    std::fs::create_dir(&linked).unwrap();
    let desk = FolderDesk::new(SharedStore::new(temp.path().join("connections.json")));
    std::fs::write(linked.join("keep.bin"), b"model").unwrap();
    let item = desk.add(&linked, None).unwrap().folder;
    assert!(!item.default);
    assert!(download_target(&desk.list().unwrap(), "checkpoints").is_none());
    let suggested = temp.path().join("new-models");
    let default = desk.ensure_default_at(&suggested).unwrap();
    assert_eq!(default.path, suggested.to_str().unwrap());
    assert_ne!(default.id, item.id);
    assert_eq!(std::fs::read(linked.join("keep.bin")).unwrap(), b"model");
    desk.remove(&default.id).unwrap();
    assert!(download_target(&desk.list().unwrap(), "checkpoints").is_none());
    desk.set_default(&item.id).unwrap();
    assert_eq!(
        download_target(&desk.list().unwrap(), "checkpoints")
            .unwrap()
            .0
            .id,
        item.id
    );
}

/// Linking another folder leaves an existing default unchanged across readers.
#[test]
fn linking_a_second_folder_preserves_the_existing_default() {
    let temp = tempfile::tempdir().unwrap();
    let desk = FolderDesk::new(SharedStore::new(temp.path().join("connections.json")));
    let existing = desk
        .add_new(&temp.path().join("install"), None)
        .unwrap()
        .folder;
    let linked = temp.path().join("discovery");
    std::fs::create_dir(&linked).unwrap();
    let item = desk.add(&linked, None).unwrap().folder;
    assert!(!item.default);
    let other = FolderDesk::new(SharedStore::new(temp.path().join("connections.json")));
    assert_eq!(
        download_target(&other.list().unwrap(), "checkpoints")
            .unwrap()
            .0
            .id,
        existing.id
    );
}
