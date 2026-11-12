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
