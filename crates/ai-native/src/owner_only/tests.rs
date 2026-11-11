//! Windows access-list checks, read back as SDDL (locale-neutral, unlike account names).

use super::*;
use crate::fsutil;
use std::fs;

/// `path`'s whole security descriptor as SDDL, which is locale-neutral, read with PowerShell.
fn sddl(path: &Path) -> String {
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "(Get-Acl -LiteralPath $env:CRAFT_ACL_PATH).Sddl",
        ])
        .env("CRAFT_ACL_PATH", path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "Get-Acl failed for {}",
        path.display()
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// The DACL part of `path`'s security descriptor, such as `D:P(A;;FA;;;S-1-5-21-...)(A;;FA;;;SY)`.
fn dacl(path: &Path) -> String {
    let sddl = sddl(path);
    let start = sddl.find("D:").unwrap();
    let end = sddl[start..].find("S:").map_or(sddl.len(), |i| start + i);
    sddl[start..end].to_owned()
}

/// The SID of the user running the tests.
fn me() -> String {
    super::win_acl::user_sid().unwrap()
}

/// The two names a private object grants, sorted like [`trustees`].
fn expected() -> Vec<String> {
    let mut t = vec![me(), "SY".to_owned()];
    t.sort();
    t
}

/// The owner SID from `path`'s security descriptor.
fn owner(path: &Path) -> String {
    let sddl = sddl(path);
    let start = sddl.find("O:").map_or(0, |i| i + 2);
    let end = sddl[start..].find("G:").map_or(sddl.len(), |i| start + i);
    sddl[start..end].to_owned()
}

#[test]
fn the_user_sid_matches_whoami() {
    let out = std::process::Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let sid = text.trim().rsplit(',').next().unwrap().trim_matches('"');
    assert_eq!(me(), sid);
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
    assert_eq!(trustees(&d), expected(), "{}: {d}", path.display());
    assert_eq!(owner(path), me(), "{}", path.display());
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
        assert_eq!(trustees(&list), expected(), "{}: {list}", d.display());
        assert_eq!(owner(d), me(), "{}", d.display());
    }
    // A file made any other way inside (history, a backup copy, a lock) inherits only those two.
    let log = inner.join("jobs.jsonl");
    fs::write(&log, b"{}\n").unwrap();
    assert_eq!(trustees(&dacl(&log)), expected());
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
