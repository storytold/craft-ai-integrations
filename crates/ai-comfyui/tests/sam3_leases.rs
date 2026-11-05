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
    let a = prepared(&server, dir.path());
    let b = prepared(&server, dir.path());
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
    let mut held: Vec<Sam3Session> = (0..4).map(|_| prepared(&server, dir.path())).collect();
    let mut slots: Vec<_> = held.iter().map(|s| s.slot().unwrap()).collect();
    slots.sort_unstable();
    slots.dedup();
    assert_eq!(slots.len(), 4, "four distinct slots");
    let mut fifth = session(&server, dir.path());
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
    let mut a = prepared(&server, dir.path());
    let slot = a.slot().unwrap();
    a.release();
    assert_eq!(a.slot(), None);
    assert!(a.preview(1.0, 1.0).is_err(), "released means unprepared");
    let b = prepared(&server, dir.path());
    assert_eq!(b.slot(), Some(slot));
}

#[test]
fn preparing_again_keeps_the_same_slot() {
    let dir = tempfile::tempdir().unwrap();
    let server = hover_server();
    let mut a = prepared(&server, dir.path());
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
    let mut bad = session(&server, dir.path());
    assert!(bad.prepare(b"not a png").is_err());
    assert_eq!(bad.slot(), None);
    let _four: Vec<_> = (0..4).map(|_| prepared(&server, dir.path())).collect();
}

#[test]
fn each_server_has_its_own_pool() {
    let dir = tempfile::tempdir().unwrap();
    let (one, two) = (hover_server(), hover_server());
    let a = prepared(&one, dir.path());
    let b = prepared(&two, dir.path());
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
            .env("CRAFT_LEASE_CHILD_DIR", dir.path())
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
    let mine = prepared(&server, dir.path());
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
    let next = prepared(&server, dir.path());
    assert_eq!(
        next.slot(),
        Some(held),
        "the dead holder's slot is reusable"
    );
}
