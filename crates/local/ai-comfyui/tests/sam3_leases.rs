//! Bounded ownership of the server's hover cache. The installed interactive node keeps one
//! cached image per node id, with no eviction and no owner check, so each prepared image must
//! hold an exclusive slot (a lock the operating system frees if its holder dies) and the number
//! of slots is bounded. Sessions in other processes are real: one test re-runs this binary.
mod sam3_support;
use craft_ai_comfyui::Sam3Session;
use sam3_support::{Reply, Server, sized_mask};
use serde_json::json;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn png() -> Vec<u8> {
    sized_mask(8, 8, &[255; 64])
}

/// A ComfyUI that accepts any prepare and answers the hover route.
fn hover_server() -> Server {
    Server::start(move |r| match r.path.as_str() {
        "/object_info" => Reply::Json(
            200,
            json!({"LoadImage":{},"LoadSAM3Model":{},"SAM3InteractiveCollector":{},
                   "MaskToImage":{},"PreviewImage":{}})
            .to_string(),
        ),
        "/upload/image" => Reply::Json(
            200,
            json!({"name":"source.png","type":"temp","subfolder":""}).to_string(),
        ),
        "/prompt" => Reply::Json(200, json!({"prompt_id":"lease-1"}).to_string()),
        "/history/lease-1" => Reply::Json(
            200,
            json!({"lease-1":{"status":{"completed":true},"outputs":{}}}).to_string(),
        ),
        "/sam3/interactive_segment_one" => {
            Reply::Json(200, json!({"num_masks":1,"overlay":"/9j/2Q=="}).to_string())
        }
        _ => Reply::Json(404, "{}".into()),
    })
}

fn session(server: &Server, dir: &Path) -> Sam3Session {
    Sam3Session::new(server.url.trim_end_matches("/v1"))
        .unwrap()
        .with_lock_dir(dir.to_path_buf())
}

/// A lock folder the session creates, as the default one is. The temp folder itself can let other
/// local accounts in, and a lock folder like that is refused.
fn lock_dir(root: &tempfile::TempDir) -> PathBuf {
    root.path().join("locks")
}

fn prepared(server: &Server, dir: &Path) -> Sam3Session {
    let mut s = session(server, dir);
    s.prepare(&png()).unwrap();
    s
}

/// The node id used by the collector in each queued graph, in queue order.
fn collector_ids(server: &Server) -> Vec<String> {
    server
        .recorded()
        .into_iter()
        .filter(|r| r.path == "/prompt")
        .map(|r| {
            let graph = r.body_json()["prompt"].clone();
            graph
                .as_object()
                .unwrap()
                .iter()
                .find(|(_, v)| v["class_type"] == "SAM3InteractiveCollector")
                .map(|(k, _)| k.clone())
                .unwrap()
        })
        .collect()
}

fn hover_ids(server: &Server) -> Vec<String> {
    server
        .recorded()
        .into_iter()
        .filter(|r| r.path == "/sam3/interactive_segment_one")
        .map(|r| r.body_json()["node_id"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn two_sessions_on_one_server_hold_different_slots_and_hover_with_their_own() {
    let dir = tempfile::tempdir().unwrap();
    let server = hover_server();
    let a = prepared(&server, &lock_dir(&dir));
    let b = prepared(&server, &lock_dir(&dir));
    assert_ne!(a.slot(), b.slot());
    assert!(a.slot().is_some() && b.slot().is_some());
    let ids = collector_ids(&server);
    assert_eq!(ids.len(), 2);
    assert_ne!(
        ids[0], ids[1],
        "no shared node id, so no shared cache entry"
    );
    assert!(ids.iter().all(|id| id.starts_with("craft_ai_sam3_")));
    a.preview(1.0, 1.0).unwrap();
    b.preview(2.0, 2.0).unwrap();
    assert_eq!(hover_ids(&server), ids, "each hover names its own slot");
}

#[test]
fn slots_are_bounded_and_a_dropped_session_frees_its_slot() {
    let dir = tempfile::tempdir().unwrap();
    let server = hover_server();
    let mut held: Vec<Sam3Session> = (0..4).map(|_| prepared(&server, &lock_dir(&dir))).collect();
    let mut slots: Vec<_> = held.iter().map(|s| s.slot().unwrap()).collect();
    slots.sort_unstable();
    slots.dedup();
    assert_eq!(slots.len(), 4, "four distinct slots");
    let mut fifth = session(&server, &lock_dir(&dir));
    let err = fifth.prepare(&png()).unwrap_err();
    assert!(err.message.contains("slots"), "{}", err.message);
    assert_eq!(fifth.slot(), None);
    let queued_before = collector_ids(&server).len();
    assert_eq!(queued_before, 4, "a refused prepare queues nothing");
    let freed = held.remove(1).slot().unwrap();
    // Dropping the removed session released its lock.
    fifth.prepare(&png()).unwrap();
    assert_eq!(fifth.slot(), Some(freed));
}

#[test]
fn release_gives_the_slot_back_and_leaves_the_session_unprepared() {
    let dir = tempfile::tempdir().unwrap();
    let server = hover_server();
    let mut a = prepared(&server, &lock_dir(&dir));
    let slot = a.slot().unwrap();
    a.release();
    assert_eq!(a.slot(), None);
    assert!(a.preview(1.0, 1.0).is_err(), "released means unprepared");
    let b = prepared(&server, &lock_dir(&dir));
    assert_eq!(b.slot(), Some(slot));
}

#[test]
fn preparing_again_keeps_the_same_slot() {
    let dir = tempfile::tempdir().unwrap();
    let server = hover_server();
    let mut a = prepared(&server, &lock_dir(&dir));
    let first = a.slot();
    a.prepare(&png()).unwrap();
    assert_eq!(a.slot(), first);
    let ids = collector_ids(&server);
    assert_eq!(ids[0], ids[1]);
}

#[test]
fn a_rejected_image_does_not_take_a_slot() {
    let dir = tempfile::tempdir().unwrap();
    let server = hover_server();
    let mut bad = session(&server, &lock_dir(&dir));
    assert!(bad.prepare(b"not a png").is_err());
    assert_eq!(bad.slot(), None);
    let _four: Vec<_> = (0..4).map(|_| prepared(&server, &lock_dir(&dir))).collect();
}

#[test]
fn each_server_has_its_own_pool() {
    let dir = tempfile::tempdir().unwrap();
    let (one, two) = (hover_server(), hover_server());
    let a = prepared(&one, &lock_dir(&dir));
    let b = prepared(&two, &lock_dir(&dir));
    assert_eq!(a.slot(), Some(0));
    assert_eq!(b.slot(), Some(0), "another server, another cache");
}

#[test]
fn the_lock_directory_is_created_on_demand_and_an_unusable_one_fails_cleanly() {
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("a").join("b");
    let server = hover_server();
    let a = prepared(&server, &nested);
    assert!(nested.is_dir());
    drop(a);
    let file = root.path().join("not-a-dir");
    std::fs::write(&file, b"x").unwrap();
    let mut s = session(&server, &file);
    let err = s.prepare(&png()).unwrap_err();
    assert!(err.message.contains("lock"), "{}", err.message);
    assert_eq!(s.slot(), None);
}

/// The lock file names a session uses for `server`, as the session derives them.
#[cfg(unix)]
fn lock_name(server: &Server, slot: usize) -> String {
    let name: String = server
        .url
        .trim_end_matches("/v1")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("slot-{name}-{slot}.lock")
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777
}

#[cfg(unix)]
#[test]
fn the_lock_folder_and_its_files_are_private_to_the_user() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("sam3-locks");
    let server = hover_server();
    let _held = prepared(&server, &dir);
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join(lock_name(&server, 0))), 0o600);
}

/// A lock file left readable by others (an older version made them that way) lets another
/// account hold the slot, so it is made private before it is used.
#[cfg(unix)]
#[test]
fn an_existing_lock_file_others_can_open_is_made_private() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("sam3-locks");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let server = hover_server();
    let lock = dir.join(lock_name(&server, 0));
    std::fs::write(&lock, b"").unwrap();
    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o644)).unwrap();
    let held = prepared(&server, &dir);
    assert_eq!(held.slot(), Some(0));
    assert_eq!(mode(&lock), 0o600);
}

/// A folder others can write is where another account plants links or holds every slot.
#[cfg(unix)]
#[test]
fn a_lock_folder_others_can_write_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("shared");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    let server = hover_server();
    let mut s = session(&server, &dir);
    let err = s.prepare(&png()).unwrap_err();
    assert!(err.message.contains("lock folder"), "{}", err.message);
    assert_eq!(s.slot(), None);
    assert!(
        std::fs::read_dir(&dir).unwrap().next().is_none(),
        "nothing created"
    );
}

#[cfg(unix)]
#[test]
fn a_lock_folder_that_is_a_link_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = root.path().join("locks");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let server = hover_server();
    let mut s = session(&server, &link);
    let err = s.prepare(&png()).unwrap_err();
    assert!(err.message.contains("lock folder"), "{}", err.message);
    assert!(
        std::fs::read_dir(&real).unwrap().next().is_none(),
        "nothing created"
    );
}

#[cfg(unix)]
#[test]
fn a_link_at_a_lock_file_name_is_not_followed() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("sam3-locks");
    std::fs::create_dir(&dir).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let server = hover_server();
    // A link to a file that exists. Followed, it would be locked and made private.
    let existing = outside.path().join("locked-through-the-link");
    std::fs::write(&existing, b"").unwrap();
    std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::os::unix::fs::symlink(&existing, dir.join(lock_name(&server, 0))).unwrap();
    let mut s = session(&server, &dir);
    assert!(
        s.prepare(&png()).is_err(),
        "a slot was taken through the link"
    );
    assert_eq!(mode(&existing), 0o644, "the linked file was changed");
}

/// A folder that already existed (one `CRAFT_AI_DIR` names) and lets every account in is where
/// another account holds every slot. `icacls` adds Everyone, as a shared folder would have.
#[cfg(windows)]
#[test]
fn a_lock_folder_other_accounts_can_use_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("shared");
    std::fs::create_dir(&dir).unwrap();
    let granted = Command::new("icacls")
        .arg(&dir)
        .args(["/grant", "*S-1-1-0:(OI)(CI)M"])
        .output()
        .unwrap();
    assert!(granted.status.success(), "{granted:?}");
    let server = hover_server();
    let mut s = session(&server, &dir);
    let err = s.prepare(&png()).unwrap_err();
    assert!(err.message.contains("lock folder"), "{}", err.message);
    assert_eq!(s.slot(), None);
    assert!(
        std::fs::read_dir(&dir).unwrap().next().is_none(),
        "nothing created"
    );
}

/// A junction needs no special right to make, so it stands in for any link at the folder name.
#[cfg(windows)]
#[test]
fn a_lock_folder_that_is_a_junction_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = root.path().join("locks");
    let made = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&real)
        .output()
        .unwrap();
    assert!(made.status.success(), "{made:?}");
    let server = hover_server();
    let mut s = session(&server, &link);
    let err = s.prepare(&png()).unwrap_err();
    assert!(err.message.contains("lock folder"), "{}", err.message);
    assert!(
        std::fs::read_dir(&real).unwrap().next().is_none(),
        "nothing created"
    );
}

/// Child half of the cross-process test: hold a slot until stdin closes.
#[test]
#[ignore = "run only as a child of the cross-process test"]
fn child_holds_a_slot_until_its_input_closes() {
    let (Ok(url), Ok(dir)) = (
        std::env::var("CRAFT_LEASE_CHILD_URL"),
        std::env::var("CRAFT_LEASE_CHILD_DIR"),
    ) else {
        return;
    };
    let mut s = Sam3Session::new(&url)
        .unwrap()
        .with_lock_dir(PathBuf::from(dir));
    s.prepare(&png()).unwrap();
    println!("HELD {}", s.slot().unwrap());
    let mut sink = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut sink);
}

/// Kills the child on every exit path so a failed assertion cannot leave it holding a slot.
struct Child(std::process::Child);

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_slot_held_by_another_process_is_skipped_and_freed_when_that_process_dies() {
    let dir = tempfile::tempdir().unwrap();
    let server = hover_server();
    let base = server.url.trim_end_matches("/v1").to_owned();
    let mut child = Child(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "child_holds_a_slot_until_its_input_closes",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("CRAFT_LEASE_CHILD_URL", &base)
            .env("CRAFT_LEASE_CHILD_DIR", lock_dir(&dir))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            // The harness prints "test name ... " without a newline, so the marker follows it.
            if let Some(at) = line.find("HELD ") {
                let _ = tx.send(line[at + 5..].trim().parse::<usize>().unwrap());
                return;
            }
        }
    });
    let held = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the child never reported holding a slot");
    let mine = prepared(&server, &lock_dir(&dir));
    assert_ne!(
        mine.slot(),
        Some(held),
        "the other process's slot is not mine"
    );
    // The holder dies without cleaning up; the operating system releases its lock.
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    drop(mine);
    // Both slots are free again, and the lowest free one is the dead holder's.
    let next = prepared(&server, &lock_dir(&dir));
    assert_eq!(
        next.slot(),
        Some(held),
        "the dead holder's slot is reusable"
    );
}
