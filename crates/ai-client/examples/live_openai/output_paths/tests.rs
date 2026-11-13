//! Output preflight uses only temporary files, never a key store or a real service.

use super::validate;
use std::path::PathBuf;

/// A pretend shared store with bytes that every refusal must preserve.
fn store() -> (tempfile::TempDir, PathBuf, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("connections.json");
    let bytes = br#"{"connections":[],"future":{"keep":true}}"#.to_vec();
    std::fs::write(&path, &bytes).unwrap();
    (dir, path, bytes)
}

/// The append destination must not be the saved connections file.
#[test]
fn a_record_cannot_append_to_the_shared_store() {
    let (_dir, path, before) = store();
    assert!(validate(&path, Some(&path), None).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// A generated image must not replace the saved connections file either.
#[test]
fn an_image_cannot_replace_the_shared_store() {
    let (_dir, path, before) = store();
    assert!(validate(&path, None, Some(&path)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// Different names for the same stored bytes are still a collision.
#[test]
fn a_record_hardlink_to_the_shared_store_is_refused() {
    let (dir, path, before) = store();
    let alias = dir.path().join("runs.jsonl");
    std::fs::hard_link(&path, &alias).unwrap();
    assert!(validate(&path, Some(&alias), None).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// Truncating a hardlink image destination would truncate the real store.
#[test]
fn an_image_hardlink_to_the_shared_store_is_refused() {
    let (dir, path, before) = store();
    let alias = dir.path().join("picture.png");
    std::fs::hard_link(&path, &alias).unwrap();
    assert!(validate(&path, None, Some(&alias)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// The existing record/image separation also applies to hardlink aliases.
#[test]
fn record_and_image_hardlinks_are_refused() {
    let (dir, path, before) = store();
    let record = dir.path().join("runs.jsonl");
    let image = dir.path().join("picture.png");
    std::fs::write(&record, b"existing evidence\n").unwrap();
    std::fs::hard_link(&record, &image).unwrap();
    assert!(validate(&path, Some(&record), Some(&image)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read(&record).unwrap(), b"existing evidence\n");
}

/// A symbolic-link name is not a separate output when it points at the store.
#[cfg(unix)]
#[test]
fn symbolic_links_to_the_store_are_refused() {
    let (dir, path, before) = store();
    let alias = dir.path().join("output-link");
    std::os::unix::fs::symlink(&path, &alias).unwrap();
    assert!(validate(&path, Some(&alias), None).is_err());
    assert!(validate(&path, None, Some(&alias)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// Existing, independent destinations and a not-yet-created destination remain usable.
#[test]
fn distinct_output_files_are_accepted_without_writing_them() {
    let (dir, path, before) = store();
    let record = dir.path().join("runs.jsonl");
    let image = dir.path().join("picture.png");
    std::fs::write(&record, b"evidence\n").unwrap();
    assert!(validate(&path, Some(&record), Some(&image)).is_ok());
    assert!(!image.exists());
    assert_eq!(std::fs::read(&record).unwrap(), b"evidence\n");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(validate(&path, None, None).is_ok());
}

/// Two names for the same not-yet-created output must still be refused.
#[test]
fn the_same_new_record_and_image_path_is_refused() {
    let (dir, path, before) = store();
    let output = dir.path().join("new-output");
    assert!(validate(&path, Some(&output), Some(&output)).is_err());
    assert!(!output.exists());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// A Windows file another operation holds without sharing cannot be proved safe to write.
#[cfg(windows)]
#[test]
fn an_unreadable_output_identity_is_refused() {
    use std::os::windows::fs::OpenOptionsExt;
    let (dir, path, before) = store();
    let output = dir.path().join("locked-output");
    std::fs::write(&output, b"other operation").unwrap();
    let _held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&output)
        .unwrap();
    assert!(validate(&path, Some(&output), None).is_err());
    assert!(validate(&path, None, Some(&output)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// A link cycle has no readable file identity; it must not be mistaken for a missing output.
#[cfg(unix)]
#[test]
fn an_unreadable_output_identity_is_refused() {
    let (dir, path, before) = store();
    let output = dir.path().join("cyclic-output");
    std::os::unix::fs::symlink(&output, &output).unwrap();
    assert!(validate(&path, Some(&output), None).is_err());
    assert!(validate(&path, None, Some(&output)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// A missing record and image named through different parent paths (`sub/..` here; a linked
/// folder behaves the same) are one file once the parent folder is resolved.
#[test]
fn outputs_named_through_different_parents_collide() {
    let (dir, path, before) = store();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let record = dir.path().join("sub").join("..").join("out.bin");
    let image = dir.path().join("out.bin");
    assert!(validate(&path, Some(&record), Some(&image)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
