//! Windows access-list checks, read back as SDDL (locale-neutral, unlike `icacls` names).

use super::*;
use crate::fsutil;
use std::fs;
use std::path::PathBuf;

/// The system's `icacls.exe`, used here only to read access lists back.
fn icacls() -> PathBuf {
    let root =
        std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    root.join("System32").join("icacls.exe")
}

/// The DACL part of `path`'s security descriptor, such as `D:P(A;;FA;;;OW)(A;;FA;;;SY)`.
fn dacl(path: &Path) -> String {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("acl.txt");
    let status = std::process::Command::new(icacls())
        .arg(path)
        .arg("/save")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "icacls /save failed for {}",
        path.display()
    );
    let raw = fs::read(&out).unwrap();
    let wide: Vec<u16> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let text = String::from_utf16_lossy(&wide);
    let sddl = text.lines().nth(1).unwrap_or_default().trim().to_owned();
    let start = sddl.find("D:").expect("no DACL in SDDL");
    let end = sddl[start..].find("S:").map_or(sddl.len(), |i| start + i);
    sddl[start..end].to_owned()
}

/// `(flags, trustee)` of every ACE in a DACL string.
fn aces(dacl: &str) -> Vec<(String, String)> {
    dacl.split('(')
        .skip(1)
        .map(|ace| {
            let parts: Vec<&str> = ace.trim_end_matches(')').split(';').collect();
            (parts[1].to_owned(), parts[5].to_owned())
        })
        .collect()
}

fn trustees(dacl: &str) -> Vec<String> {
    let mut t: Vec<String> = aces(dacl).into_iter().map(|(_, who)| who).collect();
    t.sort();
    t
}

/// Owner and SYSTEM only, set on the object itself rather than inherited.
fn assert_owner_only(path: &Path) {
    let d = dacl(path);
    assert!(
        d.starts_with("D:P"),
        "{} still inherits: {d}",
        path.display()
    );
    assert_eq!(trustees(&d), ["OW", "SY"], "{}: {d}", path.display());
    assert!(
        aces(&d).iter().all(|(flags, _)| !flags.contains("ID")),
        "{}: {d}",
        path.display()
    );
}

/// A folder under the temp folder inherits a list that is not owner-only, like a new folder
/// under `C:\`. This checks the test's own premise.
fn inheriting_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    assert!(!dacl(dir.path()).starts_with("D:P"));
    dir
}

#[test]
fn atomic_write_makes_the_file_owner_only_in_an_inheriting_folder() {
    let dir = inheriting_dir();
    let p = dir.path().join("connections.json");
    fsutil::atomic_write(&p, b"{}").unwrap();
    assert_owner_only(&p);
    // A replacement keeps the private list.
    fsutil::atomic_write(&p, b"{\"a\":1}").unwrap();
    assert_owner_only(&p);
    assert_eq!(fs::read(&p).unwrap(), b"{\"a\":1}");
}

#[test]
fn a_leftover_temp_file_is_replaced_not_reused() {
    let dir = inheriting_dir();
    let p = dir.path().join("secrets.enc.json");
    let tmp = dir
        .path()
        .join(format!(".secrets.enc.json.tmp-{}", std::process::id()));
    fs::write(&tmp, b"stale").unwrap();
    fsutil::atomic_write(&p, b"new").unwrap();
    assert_eq!(fs::read(&p).unwrap(), b"new");
    assert_owner_only(&p);
    assert!(!tmp.exists());
}

#[test]
fn new_folders_are_owner_only_and_pass_it_on() {
    let dir = inheriting_dir();
    let outer = dir.path().join("craft");
    let inner = outer.join("ai");
    create_dir_all(&inner).unwrap();
    for d in [&outer, &inner] {
        let list = dacl(d);
        assert!(list.starts_with("D:P"), "{}: {list}", d.display());
        assert_eq!(trustees(&list), ["OW", "SY"], "{}: {list}", d.display());
    }
    // A file made any other way inside (history, a backup copy, a lock) inherits only those two.
    let log = inner.join("jobs.jsonl");
    fs::write(&log, b"{}\n").unwrap();
    assert_eq!(trustees(&dacl(&log)), ["OW", "SY"]);
    // The folder that already existed is left alone.
    assert!(!dacl(dir.path()).starts_with("D:P"));
}

#[test]
fn an_existing_folder_is_not_changed() {
    let dir = inheriting_dir();
    let before = dacl(dir.path());
    create_dir_all(dir.path()).unwrap();
    assert_eq!(dacl(dir.path()), before);
}

#[test]
fn an_existing_file_is_never_taken_over() {
    let dir = inheriting_dir();
    let p = dir.path().join("planted");
    fs::write(&p, b"x").unwrap();
    let before = dacl(&p);
    let err = create_file(&p).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(dacl(&p), before);
}

#[test]
fn no_other_handle_can_open_the_file_while_it_is_being_written() {
    let dir = inheriting_dir();
    let p = dir.path().join("secret");
    let held = create_file(&p).unwrap();
    assert!(fs::File::open(&p).is_err());
    drop(held);
    assert_owner_only(&p);
    assert!(fs::File::open(&p).is_ok());
}
