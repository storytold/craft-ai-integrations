//! Unix modes of what this crate creates, whatever the process umask.

use super::*;
use crate::HistoryFile;
use craft_ai_core::HistoryRecord;
use craft_ai_core::history::{EndRecord, HISTORY_SCHEMA, Outcome};
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn record() -> HistoryRecord {
    HistoryRecord::End(EndRecord {
        v: HISTORY_SCHEMA,
        run: "a".into(),
        at_ms: 1,
        outcome: Outcome::Done,
        usage: None,
    })
}

#[test]
fn new_folders_are_private_and_existing_ones_are_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let outer = dir.path().join("ArtCraft");
    let inner = outer.join("ai");
    create_dir_all(&inner).unwrap();
    assert_eq!(mode(&outer), 0o700);
    assert_eq!(mode(&inner), 0o700);
    assert_eq!(mode(dir.path()), 0o755);
}

/// A folder this crate did not create (one `CRAFT_AI_DIR` names) still gets a private log.
#[test]
fn a_new_history_file_in_an_existing_folder_is_private() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jobs.jsonl");
    HistoryFile::new(&path).append(&record()).unwrap();
    assert_eq!(mode(&path), 0o600);
}

#[test]
fn write_atomic_makes_a_private_file_in_a_private_new_folder() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("comfy").join("extra_model_paths.yaml");
    write_atomic(
        &path, b"a: b
",
    )
    .unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        b"a: b
"
    );
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
}
