//! Per-app consent: the gateway reads the launcher's app list once, at start, so an app the
//! person takes off the list later is refused by asking again (storytold/craft-ai-integrations#208).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::hub_tests::{entry, hub, slow_conn, text};
use super::*;

/// The ids the list names now; tests take apps off it and put them back.
type Listed = Arc<Mutex<Vec<&'static str>>>;

fn allow_from(listed: &Listed) -> Box<Allow> {
    let listed = listed.clone();
    Box::new(move |id: &str| listed.lock().unwrap().contains(&id))
}

fn says_not_offered(r: &Value) -> bool {
    r["isError"] == true
        && r["content"][0]["text"]
            .as_str()
            .is_some_and(|t| t.contains("no longer available"))
}

/// An app taken off the list is no longer listed, searched, described or called, and its
/// server is stopped; the other apps still answer, and putting it back starts it anew.
#[test]
fn an_app_taken_off_the_list_is_no_longer_offered() {
    let starts = Arc::new(AtomicUsize::new(0));
    let listed: Listed = Arc::new(Mutex::new(vec!["photocraft", "wordcraft"]));
    let h = hub(starts.clone()).with_allow(allow_from(&listed));
    let call = |name: &str, args: Value| tools::call(&h, name, &args).unwrap();
    let photo = json!({"app": "photocraft", "tool": "layer.new", "arguments": {}});
    assert_eq!(call(CALL_TOOL, photo.clone()).get("isError"), None);
    assert_eq!(starts.load(Ordering::SeqCst), 1);

    listed.lock().unwrap().retain(|id| *id != "photocraft");
    let apps = text(&call(LIST_APPS, json!({})));
    let ids: Vec<&str> = apps["apps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["app"].as_str())
        .collect();
    assert_eq!(ids, ["wordcraft"], "{apps}");
    let found = text(&call(FIND_TOOLS, json!({"query": "blur"})));
    assert_eq!(
        found["tools"],
        json!([]),
        "its known tools are not searched: {found}"
    );
    assert_eq!(
        starts.load(Ordering::SeqCst),
        2,
        "only WordCraft was started"
    );
    let one = call(FIND_TOOLS, json!({"query": "blur", "app": "photocraft"}));
    assert!(says_not_offered(&one), "{one}");
    let described = call(
        DESCRIBE_TOOL,
        json!({"app": "photocraft", "tool": "filter.gaussian_blur"}),
    );
    assert!(says_not_offered(&described), "{described}");
    let refused = call(CALL_TOOL, photo.clone());
    assert!(says_not_offered(&refused), "{refused}");
    assert_eq!(
        starts.load(Ordering::SeqCst),
        2,
        "PhotoCraft was not started again"
    );

    let word = json!({"app": "wordcraft", "tool": "doc.new", "arguments": {}});
    assert_eq!(call(CALL_TOOL, word).get("isError"), None);

    listed.lock().unwrap().push("photocraft");
    assert_eq!(call(CALL_TOOL, photo).get("isError"), None);
    assert_eq!(
        starts.load(Ordering::SeqCst),
        3,
        "its old server was stopped, so it is started anew"
    );
}

/// One slow PhotoCraft whose consent is `listed`, and the gate its calls wait at: `entered`
/// says a call arrived, `release` lets it finish, `calls` counts the calls that reached it.
struct Slow {
    hub: Arc<Hub>,
    entered: std::sync::mpsc::Receiver<()>,
    release: std::sync::mpsc::Sender<()>,
    calls: Arc<AtomicUsize>,
}

fn slow(listed: &Listed) -> Slow {
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let hub = Hub::with_connector(
        vec![entry("photocraft", "PhotoCraft")],
        vec![],
        Box::new(move |_| {
            Ok(slow_conn(
                entered_tx.clone(),
                release_rx.clone(),
                counted.clone(),
            ))
        }),
    )
    .with_allow(allow_from(listed));
    Slow {
        hub: Arc::new(hub),
        entered,
        release,
        calls,
    }
}

/// Starts a call to PhotoCraft's `layer.new` on another thread.
fn start_call(h: &Arc<Hub>) -> std::thread::JoinHandle<Result<Value, String>> {
    let h = h.clone();
    std::thread::spawn(move || {
        h.call_tool(
            "photocraft",
            "layer.new",
            json!({}),
            Duration::from_secs(20),
        )
    })
}

/// Whether PhotoCraft's server runs, asked while the list names it.
fn started(h: &Hub, listed: &Listed) -> Value {
    listed.lock().unwrap().push("photocraft");
    h.list_apps()["apps"][0]["started"].clone()
}

/// A call that was running when its app was taken off the list finishes, then the app's
/// server is stopped.
#[test]
fn a_running_call_finishes_then_its_app_is_stopped() {
    let listed: Listed = Arc::new(Mutex::new(vec!["photocraft"]));
    let s = slow(&listed);
    let a = start_call(&s.hub);
    s.entered.recv_timeout(Duration::from_secs(5)).unwrap();
    listed.lock().unwrap().clear();
    s.release.send(()).unwrap();
    assert!(a.join().unwrap().is_ok(), "the running call finishes");
    assert_eq!(
        started(&s.hub, &listed),
        false,
        "then its server is stopped"
    );
}

/// A call that waited for the app while it was taken off the list never reaches it.
#[test]
fn a_call_waiting_for_an_app_taken_off_the_list_never_reaches_it() {
    let listed: Listed = Arc::new(Mutex::new(vec!["photocraft"]));
    let s = slow(&listed);
    let a = start_call(&s.hub);
    s.entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let b = start_call(&s.hub);
    // B passed the first check and waits for the app A holds.
    std::thread::sleep(Duration::from_millis(300));
    assert!(!b.is_finished(), "B is queued behind A");
    listed.lock().unwrap().clear();
    s.release.send(()).unwrap();
    assert!(a.join().unwrap().is_ok());
    let b = b.join().unwrap().unwrap_err();
    assert!(b.contains("no longer available"), "{b}");
    assert_eq!(s.calls.load(Ordering::SeqCst), 1, "B never reached the app");
}
