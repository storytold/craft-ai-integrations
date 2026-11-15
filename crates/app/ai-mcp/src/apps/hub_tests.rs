use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

use super::child::tests::fake;
use super::*;

fn entry(id: &str, name: &str) -> AppEntry {
    AppEntry {
        id: id.into(),
        name: name.into(),
        command: PathBuf::from("/fake"),
        args: vec!["mcp".into()],
    }
}

/// A fake app with `tools`; `tools/call` echoes the tool name and arguments.
fn app_conn(tools: Value) -> Conn {
    fake(move |m, req| {
        let r = match m {
            "tools/list" => json!({"tools": tools}),
            "tools/call" => {
                json!({"content": [{"type": "text", "text": format!("{} {}", req["params"]["name"], req["params"]["arguments"])}]})
            }
            _ => return None,
        };
        Some(vec![
            json!({"jsonrpc": "2.0", "id": req["id"], "result": r}).to_string(),
        ])
    })
}

/// Two fake apps; `starts` counts connections made.
fn hub(starts: Arc<AtomicUsize>) -> Hub {
    Hub::with_connector(
        vec![
            entry("photocraft", "PhotoCraft"),
            entry("wordcraft", "WordCraft"),
        ],
        vec![],
        Box::new(move |app| {
            starts.fetch_add(1, Ordering::SeqCst);
            Ok(match app.id.as_str() {
                "photocraft" => app_conn(json!([
                    {"name": "filter.gaussian_blur", "description": "Blur the layer.", "inputSchema": {"type": "object", "properties": {"radius": {"type": "number"}}}},
                    {"name": "layer.new", "description": "Add a layer."}])),
                _ => app_conn(json!([{"name": "doc.new", "description": "New document."}])),
            })
        }),
    )
}

fn text(v: &Value) -> Value {
    serde_json::from_str(v["content"][0]["text"].as_str().unwrap()).unwrap_or(Value::Null)
}

#[test]
fn listing_apps_starts_nothing() {
    let starts = Arc::new(AtomicUsize::new(0));
    let h = hub(starts.clone());
    let out = tools::call(&h, LIST_APPS, &json!({})).unwrap();
    let apps = text(&out)["apps"].as_array().unwrap().clone();
    assert_eq!(apps.len(), 2);
    assert_eq!(apps[0]["app"], "photocraft");
    assert_eq!(apps[0]["started"], false);
    assert_eq!(starts.load(Ordering::SeqCst), 0);
}

#[test]
fn find_in_one_app_starts_only_that_app_and_caches_its_tools() {
    let starts = Arc::new(AtomicUsize::new(0));
    let h = hub(starts.clone());
    let out = tools::call(
        &h,
        FIND_TOOLS,
        &json!({"query": "blur", "app": "photocraft"}),
    )
    .unwrap();
    let hits = text(&out)["tools"].as_array().unwrap().clone();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["tool"], "filter.gaussian_blur");
    assert!(
        hits[0].get("inputSchema").is_none(),
        "find returns no schemas"
    );
    tools::call(
        &h,
        FIND_TOOLS,
        &json!({"query": "layer", "app": "photocraft"}),
    )
    .unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    let listed = text(&tools::call(&h, LIST_APPS, &json!({})).unwrap());
    assert_eq!(listed["apps"][0]["started"], true);
    assert_eq!(listed["apps"][0]["tools"], 2);
}

#[test]
fn find_across_apps() {
    let h = hub(Arc::new(AtomicUsize::new(0)));
    let out = text(&tools::call(&h, FIND_TOOLS, &json!({"query": "new"})).unwrap());
    let apps: Vec<&str> = out["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["app"].as_str().unwrap())
        .collect();
    assert!(
        apps.contains(&"photocraft") && apps.contains(&"wordcraft"),
        "{apps:?}"
    );
}

#[test]
fn an_app_that_will_not_start_is_reported_and_the_others_still_answer() {
    let h = Hub::with_connector(
        vec![
            entry("photocraft", "PhotoCraft"),
            entry("wordcraft", "WordCraft"),
        ],
        vec![],
        Box::new(|app| match app.id.as_str() {
            "photocraft" => Err("could not start PhotoCraft".into()),
            _ => Ok(app_conn(json!([{"name": "doc.new"}]))),
        }),
    );
    let out = text(&tools::call(&h, FIND_TOOLS, &json!({"query": "doc"})).unwrap());
    assert_eq!(out["tools"][0]["app"], "wordcraft");
    assert_eq!(out["unavailable"][0]["app"], "photocraft");
    let one = tools::call(
        &h,
        FIND_TOOLS,
        &json!({"query": "doc", "app": "photocraft"}),
    )
    .unwrap();
    assert_eq!(one["isError"], true);
}

#[test]
fn describe_gives_the_schema() {
    let h = hub(Arc::new(AtomicUsize::new(0)));
    let out = text(
        &tools::call(
            &h,
            DESCRIBE_TOOL,
            &json!({"app": "photocraft", "tool": "filter.gaussian_blur"}),
        )
        .unwrap(),
    );
    assert_eq!(out["inputSchema"]["properties"]["radius"]["type"], "number");
}

#[test]
fn call_passes_the_apps_result_through() {
    let h = hub(Arc::new(AtomicUsize::new(0)));
    let out = tools::call(
        &h,
        CALL_TOOL,
        &json!({"app": "PhotoCraft", "tool": "layer.new", "arguments": {"name": "A"}}),
    )
    .unwrap();
    assert_eq!(out["content"][0]["text"], r#""layer.new" {"name":"A"}"#);
    assert!(out.get("isError").is_none());
}

#[test]
fn bad_arguments_are_tool_errors_not_panics() {
    let h = hub(Arc::new(AtomicUsize::new(0)));
    for args in [
        json!({}),
        json!({"app": 3, "tool": "x"}),
        json!({"app": "photocraft"}),
        json!({"app": "photocraft", "tool": "layer.new", "arguments": [1]}),
        json!({"app": "photocraft", "tool": "layer.new", "timeoutSeconds": 0}),
        json!({"app": "photocraft", "tool": "layer.new", "timeoutSeconds": -5}),
        json!({"app": "photocraft", "tool": "no.such"}),
        json!({"app": "nocraft", "tool": "x"}),
    ] {
        let out = tools::call(&h, CALL_TOOL, &args).unwrap();
        assert_eq!(out["isError"], true, "{args}");
    }
    for args in [
        json!({"query": 5}),
        json!({"query": "x", "limit": "ten"}),
        json!({"query": "x", "app": "nocraft"}),
    ] {
        assert_eq!(
            tools::call(&h, FIND_TOOLS, &args).unwrap()["isError"],
            true,
            "{args}"
        );
    }
    assert!(tools::call(&h, "not_ours", &json!({})).is_none());
}

#[test]
fn a_dead_connection_is_replaced() {
    let starts = Arc::new(AtomicUsize::new(0));
    let s = starts.clone();
    let h = Hub::with_connector(
        vec![entry("wordcraft", "WordCraft")],
        vec![],
        Box::new(move |_| {
            let n = s.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                // Answers tools/list, then goes silent: the call times out and closes it.
                let listed = std::sync::atomic::AtomicBool::new(false);
                let c = fake(move |m, req| {
                    if m == "tools/list" && !listed.swap(true, Ordering::SeqCst) {
                        return Some(vec![json!({"jsonrpc": "2.0", "id": req["id"], "result": {"tools": [{"name": "doc.new"}]}}).to_string()]);
                    }
                    None
                });
                Ok(c)
            } else {
                Ok(app_conn(json!([{"name": "doc.new"}])))
            }
        }),
    );
    let first = tools::call(
        &h,
        CALL_TOOL,
        &json!({"app": "wordcraft", "tool": "doc.new", "timeoutSeconds": 1}),
    )
    .unwrap();
    assert_eq!(first["isError"], true);
    let second = tools::call(
        &h,
        CALL_TOOL,
        &json!({"app": "wordcraft", "tool": "doc.new"}),
    )
    .unwrap();
    assert!(second.get("isError").is_none(), "{second}");
    assert_eq!(starts.load(Ordering::SeqCst), 2);
}

#[test]
fn no_apps_says_how_to_turn_them_on() {
    let h = Hub::with_connector(vec![], vec![], Box::new(|_| Err("unused".into())));
    let out = tools::call(&h, CALL_TOOL, &json!({"app": "photocraft", "tool": "x"})).unwrap();
    assert!(
        out["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("ArtCraft Launcher")
    );
}

#[test]
fn no_match_points_at_the_apps_command_tools() {
    let h = Hub::with_connector(
        vec![entry("photocraft", "PhotoCraft")],
        vec![],
        Box::new(|_| {
            Ok(app_conn(json!([
                {"name": "command_list", "description": "List engine commands."},
                {"name": "command_run", "description": "Run an engine command."},
                {"name": "doc_new", "description": "New document."}])))
        }),
    );
    let out = text(
        &tools::call(
            &h,
            FIND_TOOLS,
            &json!({"query": "gaussian", "app": "photocraft"}),
        )
        .unwrap(),
    );
    assert!(out["tools"].as_array().unwrap().is_empty());
    let hint = out["hint"].as_str().unwrap();
    assert!(
        hint.contains("photocraft/command_list") && hint.contains("photocraft/command_run"),
        "{hint}"
    );
    assert!(!hint.contains("doc_new"));
    let hit = text(
        &tools::call(
            &h,
            FIND_TOOLS,
            &json!({"query": "doc", "app": "photocraft"}),
        )
        .unwrap(),
    );
    assert!(hit.get("hint").is_none());
}

#[test]
fn the_tool_list_is_read_again_after_the_app_says_it_changed() {
    let lists = Arc::new(AtomicUsize::new(0));
    let l = lists.clone();
    let h = Hub::with_connector(
        vec![entry("wordcraft", "WordCraft")],
        vec![],
        Box::new(move |_| {
            let l = l.clone();
            Ok(fake(move |m, req| {
                let r = match m {
                    "tools/list" => {
                        let n = l.fetch_add(1, Ordering::SeqCst);
                        let name = if n == 0 { "doc.old" } else { "doc.new" };
                        json!({"tools": [{"name": name}]})
                    }
                    "tools/call" => {
                        return Some(vec![
                            json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
                                .to_string(),
                            json!({"jsonrpc": "2.0", "id": req["id"], "result": {"content": []}})
                                .to_string(),
                        ]);
                    }
                    _ => return None,
                };
                Some(vec![
                    json!({"jsonrpc": "2.0", "id": req["id"], "result": r}).to_string(),
                ])
            }))
        }),
    );
    tools::call(
        &h,
        CALL_TOOL,
        &json!({"app": "wordcraft", "tool": "doc.old"}),
    )
    .unwrap();
    let out =
        text(&tools::call(&h, FIND_TOOLS, &json!({"query": "doc", "app": "wordcraft"})).unwrap());
    assert_eq!(out["tools"][0]["tool"], "doc.new");
    assert_eq!(lists.load(Ordering::SeqCst), 2);
}

#[test]
fn an_app_that_stopped_has_its_tools_read_again() {
    let starts = Arc::new(AtomicUsize::new(0));
    let s = starts.clone();
    let h = Hub::with_connector(
        vec![entry("wordcraft", "WordCraft")],
        vec![],
        Box::new(move |_| {
            let n = s.fetch_add(1, Ordering::SeqCst);
            let name = if n == 0 { "doc.old" } else { "doc.new" };
            let tools = json!([{"name": name}]);
            if n == 0 {
                // Lists its tools, then exits: the stream ends.
                let (client_r, mut server_w) = std::io::pipe().unwrap();
                let (server_r, client_w) = std::io::pipe().unwrap();
                std::thread::spawn(move || {
                    use std::io::{BufRead, Write};
                    let mut lines = std::io::BufReader::new(server_r).lines();
                    if let Some(Ok(l)) = lines.next() {
                        let req: Value = serde_json::from_str(&l).unwrap();
                        let reply =
                            json!({"jsonrpc": "2.0", "id": req["id"], "result": {"tools": tools}});
                        let _ = writeln!(server_w, "{reply}");
                    }
                });
                Ok(Conn::new(Box::new(client_r), Box::new(client_w)))
            } else {
                Ok(app_conn(tools))
            }
        }),
    );
    let first =
        text(&tools::call(&h, FIND_TOOLS, &json!({"query": "doc", "app": "wordcraft"})).unwrap());
    assert_eq!(first["tools"][0]["tool"], "doc.old");
    // The app ends after its listing; wait (with a bound) until the gateway has seen that.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut again = Value::Null;
    while std::time::Instant::now() < deadline {
        again = text(
            &tools::call(&h, FIND_TOOLS, &json!({"query": "doc", "app": "wordcraft"})).unwrap(),
        );
        if again["tools"][0]["tool"] == "doc.new" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(
        again["tools"][0]["tool"], "doc.new",
        "the restarted app's tools, not the cached ones"
    );
    assert_eq!(starts.load(Ordering::SeqCst), 2);
}

/// Turning agents off in the launcher refuses every tool at once, in a gateway a client already
/// runs, and stops the app servers it started; turning them on again starts them anew.
#[test]
fn the_launchers_switch_is_asked_before_every_call() {
    use std::sync::atomic::AtomicBool;
    let starts = Arc::new(AtomicUsize::new(0));
    let on = Arc::new(AtomicBool::new(true));
    let switch = on.clone();
    let h = hub(starts.clone()).with_switch(Box::new(move || switch.load(Ordering::SeqCst)));
    let call = |name: &str, args: Value| tools::call(&h, name, &args).unwrap();
    let args = json!({"app": "photocraft", "tool": "layer.new", "arguments": {}});
    assert_eq!(call(CALL_TOOL, args.clone()).get("isError"), None);
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    on.store(false, Ordering::SeqCst);
    for (name, a) in [
        (LIST_APPS, json!({})),
        (FIND_TOOLS, json!({"query": "blur"})),
        (CALL_TOOL, args.clone()),
    ] {
        let r = call(name, a);
        assert_eq!(r["isError"], true, "{name}");
        assert!(
            r["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("turned off"),
            "{name}: {r}"
        );
    }
    on.store(true, Ordering::SeqCst);
    assert_eq!(call(CALL_TOOL, args).get("isError"), None);
    assert_eq!(
        starts.load(Ordering::SeqCst),
        2,
        "the stopped server is started anew"
    );
    assert!(
        tools::call(&h, "not_a_gateway_tool", &json!({})).is_none(),
        "other names stay unknown"
    );
}

/// A fake PhotoCraft whose `tools/call` says it arrived on `entered`, then waits for `release`;
/// `calls` counts the calls that reached it.
fn slow_conn(
    entered: std::sync::mpsc::Sender<()>,
    release: Arc<std::sync::Mutex<std::sync::mpsc::Receiver<()>>>,
    calls: Arc<AtomicUsize>,
) -> Conn {
    fake(move |m, req| {
        let r = match m {
            "tools/list" => {
                json!({"tools": [{"name": "layer.new", "description": "Add a layer."}]})
            }
            "tools/call" => {
                calls.fetch_add(1, Ordering::SeqCst);
                let _ = entered.send(());
                let _ = release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10));
                json!({"content": [{"type": "text", "text": "done"}]})
            }
            _ => return None,
        };
        Some(vec![
            json!({"jsonrpc": "2.0", "id": req["id"], "result": r}).to_string(),
        ])
    })
}

type Gate = (
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::Sender<()>,
    Arc<AtomicUsize>,
);

/// One slow app, its switch, and the gate its calls wait at.
fn slow_hub(on: Arc<std::sync::atomic::AtomicBool>) -> (Arc<Hub>, Gate) {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Arc::new(std::sync::Mutex::new(release_rx));
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
    .with_switch(Box::new(move || on.load(Ordering::SeqCst)));
    (Arc::new(hub), (entered_rx, release_tx, calls))
}

/// A long call holds the app, not the gateway: listing the apps, searching all of them and
/// describing a tool answer at once from the list read before it (storytold/craft-ai-integrations#197
/// review).
#[test]
fn a_long_call_does_not_hold_up_listing_searching_or_describing() {
    let on = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let (h, (entered, release, _)) = slow_hub(on);
    let caller = {
        let h = h.clone();
        std::thread::spawn(move || {
            h.call_tool(
                "photocraft",
                "layer.new",
                json!({}),
                Duration::from_secs(20),
            )
        })
    };
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let start = std::time::Instant::now();
    let apps = h.list_apps();
    assert_eq!(apps["apps"][0]["busy"], true, "{apps}");
    assert_eq!(apps["apps"][0]["started"], true);
    assert_eq!(apps["apps"][0]["tools"], 1);
    let found = h.find_tools("layer", None, 10).unwrap();
    assert_eq!(found["tools"][0]["tool"], "layer.new", "{found}");
    assert!(h.describe_tool("photocraft", "layer.new").is_ok());
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "waited {:?} on the running call",
        start.elapsed()
    );
    release.send(()).unwrap();
    assert!(caller.join().unwrap().is_ok());
    assert!(
        h.list_apps()["apps"][0].get("busy").is_none(),
        "not busy once the call ended"
    );
}

/// A call that waited for the app while agents were turned off never reaches it; the call that
/// was running finishes, and the app's server is stopped after it (storytold/craft-ai-integrations#197
/// review).
#[test]
fn a_call_waiting_for_the_app_never_starts_once_agents_are_turned_off() {
    let on = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let (h, (entered, release, calls)) = slow_hub(on.clone());
    let args = json!({"app": "photocraft", "tool": "layer.new", "arguments": {}});
    let call = |h: &Arc<Hub>| {
        let (h, args) = (h.clone(), args.clone());
        std::thread::spawn(move || tools::call(&h, CALL_TOOL, &args).unwrap())
    };
    let a = call(&h);
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let b = call(&h);
    // B passed the switch and waits for the app A holds.
    std::thread::sleep(Duration::from_millis(300));
    assert!(!b.is_finished(), "B is queued behind A");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    on.store(false, Ordering::SeqCst);
    release.send(()).unwrap();
    let (a, b) = (a.join().unwrap(), b.join().unwrap());
    assert!(a.get("isError").is_none(), "the running call finishes: {a}");
    assert_eq!(b["isError"], true, "{b}");
    assert!(
        b["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("turned off"),
        "{b}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "B never reached the app");
    assert_eq!(
        h.list_apps()["apps"][0]["started"],
        false,
        "the app's server is stopped after A"
    );
}

/// Turning agents off while something holds an app's connection for a moment (listing or
/// searching) stops that app's server as soon as it lets go, not at the app's next request
/// (storytold/craft-ai-integrations#197 review).
#[test]
fn an_app_held_for_a_moment_is_stopped_when_it_is_let_go() {
    use std::sync::atomic::AtomicBool;
    let on = Arc::new(AtomicBool::new(true));
    let switch = on.clone();
    let h = hub(Arc::new(AtomicUsize::new(0)))
        .with_switch(Box::new(move || switch.load(Ordering::SeqCst)));
    h.find_tools("blur", Some("photocraft"), 10).unwrap();
    let slot = &h.slots[0];
    let held = h.hold(slot, false, |conn| {
        on.store(false, Ordering::SeqCst);
        assert!(h.check_switch().is_err());
        conn.as_ref().is_some_and(|c| c.closed().is_none())
    });
    assert_eq!(held, Some(true), "still running while it was held");
    assert!(
        lock(&slot.conn).is_none(),
        "the server is stopped once the holder let go"
    );
    on.store(true, Ordering::SeqCst);
    h.find_tools("blur", Some("photocraft"), 10).unwrap();
    assert!(
        lock(&slot.conn).is_some(),
        "a stop left over from before doesn't stop the app started anew"
    );
    h.hold(slot, false, |_| ());
    assert!(lock(&slot.conn).is_some());
}

/// A call that finds its app's server gone once it gets the app starts it anew and reads the
/// new server's tool list before running: describing a tool during that call answers from it at
/// once instead of waiting for the call (storytold/craft-ai-integrations#197 review).
#[test]
fn a_call_that_restarts_its_app_does_not_hold_up_describing() {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Arc::new(std::sync::Mutex::new(release_rx));
    let calls = Arc::new(AtomicUsize::new(0));
    let (counted, starts) = (calls.clone(), Arc::new(AtomicUsize::new(0)));
    let h = Arc::new(Hub::with_connector(
        vec![entry("photocraft", "PhotoCraft")],
        vec![],
        Box::new(move |_| {
            if starts.fetch_add(1, Ordering::SeqCst) == 0 {
                // Lists its tools, then never answers a call: that call times out and the
                // server is stopped.
                Ok(fake(|m, req| {
                    (m == "tools/list").then(|| {
                        vec![json!({"jsonrpc": "2.0", "id": req["id"], "result": {"tools": [{"name": "layer.new"}]}}).to_string()]
                    })
                }))
            } else {
                Ok(slow_conn(
                    entered_tx.clone(),
                    release_rx.clone(),
                    counted.clone(),
                ))
            }
        }),
    ));
    let call = |h: &Arc<Hub>, secs| {
        let h = h.clone();
        std::thread::spawn(move || {
            h.call_tool(
                "photocraft",
                "layer.new",
                json!({}),
                Duration::from_secs(secs),
            )
        })
    };
    let a = call(&h, 1);
    // A holds the app; B finds the tool in the list read before and waits for the app.
    std::thread::sleep(Duration::from_millis(300));
    let b = call(&h, 20);
    assert!(a.join().unwrap().is_err(), "A timed out");
    // B got the app, found the server gone and started it anew.
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let start = std::time::Instant::now();
    assert!(h.describe_tool("photocraft", "layer.new").is_ok());
    assert_eq!(h.list_apps()["apps"][0]["tools"], 1);
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "waited {:?} on the running call",
        start.elapsed()
    );
    release_tx.send(()).unwrap();
    assert!(b.join().unwrap().is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
