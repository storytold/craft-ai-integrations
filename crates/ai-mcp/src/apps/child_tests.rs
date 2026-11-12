use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

use serde_json::{Value, json};

use super::*;

/// An in-process MCP server: answers each request with `reply(method, params)`; `None` stays
/// silent. Returns the client side.
pub(crate) fn fake<F>(reply: F) -> Conn
where
    F: Fn(&str, &Value) -> Option<Vec<String>> + Send + 'static,
{
    let (client_r, mut server_w) = std::io::pipe().unwrap();
    let (server_r, client_w) = std::io::pipe().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(server_r).lines() {
            let Ok(line) = line else { return };
            let Ok(req) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let method = req["method"].as_str().unwrap_or("").to_string();
            let Some(out) = reply(&method, &req) else {
                continue;
            };
            for l in out {
                if server_w.write_all(format!("{l}\n").as_bytes()).is_err() {
                    return;
                }
            }
        }
    });
    Conn::new(Box::new(client_r), Box::new(client_w))
}

fn result(req: &Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": req["id"], "result": result}).to_string()
}

#[test]
fn handshake_list_and_call() {
    let mut c = fake(|m, req| match m {
        "initialize" => Some(vec![result(req, json!({"protocolVersion": "2025-06-18"}))]),
        "notifications/initialized" => None,
        "tools/list" => Some(vec![result(
            req,
            json!({"tools": [{"name": "doc.new"}, {"no": "name"}]}),
        )]),
        "tools/call" => Some(vec![result(
            req,
            json!({"content": [{"type": "text", "text": req["params"]["name"]}]}),
        )]),
        _ => None,
    });
    c.initialize().unwrap();
    let tools = c.list_tools().unwrap();
    assert_eq!(tools.len(), 1, "a tool without a name is left out");
    let r = c
        .call_tool("doc.new", json!({}), Duration::from_secs(5))
        .unwrap();
    assert_eq!(r["content"][0]["text"], "doc.new");
}

#[test]
fn pages_are_followed() {
    let mut c = fake(|m, req| match m {
        "tools/list" if req["params"]["cursor"].is_null() => Some(vec![result(
            req,
            json!({"tools": [{"name": "a"}], "nextCursor": "p2"}),
        )]),
        "tools/list" => Some(vec![result(req, json!({"tools": [{"name": "b"}]}))]),
        _ => None,
    });
    let names: Vec<String> = c
        .list_tools()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, vec!["a", "b"]);
}

#[test]
fn notifications_log_lines_and_stale_replies_are_skipped() {
    let mut c = fake(|_, req| {
        Some(vec![
            "starting up...".to_string(),
            json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {}}).to_string(),
            json!({"jsonrpc": "2.0", "id": 999, "result": "stale"}).to_string(),
            result(req, json!("mine")),
        ])
    });
    assert_eq!(
        c.request("x", json!({}), Duration::from_secs(5)).unwrap(),
        json!("mine")
    );
}

#[test]
fn a_request_from_the_app_is_refused_and_the_reply_still_arrives() {
    let mut c = fake(|m, req| match m {
        "x" => Some(vec![
            json!({"jsonrpc": "2.0", "id": "srv-1", "method": "sampling/createMessage"})
                .to_string(),
            result(req, json!(1)),
        ]),
        _ => None,
    });
    assert_eq!(
        c.request("x", json!({}), Duration::from_secs(5)).unwrap(),
        json!(1)
    );
}

#[test]
fn an_error_reply_is_an_err_and_the_connection_stays_usable() {
    let mut c = fake(|m, req| match m {
        "bad" => Some(vec![
            json!({"jsonrpc": "2.0", "id": req["id"], "error": {"code": -1, "message": "nope"}})
                .to_string(),
        ]),
        _ => Some(vec![result(req, json!(true))]),
    });
    assert_eq!(
        c.request("bad", json!({}), Duration::from_secs(5))
            .unwrap_err(),
        "nope"
    );
    assert!(c.closed().is_none());
    assert_eq!(
        c.request("ok", json!({}), Duration::from_secs(5)).unwrap(),
        json!(true)
    );
}

#[test]
fn a_silent_app_times_out_and_the_connection_is_closed() {
    let mut c = fake(|_, _| None);
    let err = c
        .request("x", json!({}), Duration::from_millis(100))
        .unwrap_err();
    assert!(err.contains("did not answer"), "{err}");
    assert!(c.closed().is_some());
    assert!(c.request("y", json!({}), Duration::from_secs(5)).is_err());
}

#[test]
fn an_app_that_exits_is_reported() {
    let (client_r, server_w) = std::io::pipe().unwrap();
    let (_server_r, client_w) = std::io::pipe().unwrap();
    drop(server_w);
    let mut c = Conn::new(Box::new(client_r), Box::new(client_w));
    assert!(c.request("x", json!({}), Duration::from_secs(5)).is_err());
    assert!(c.closed().is_some());
}

#[test]
fn capped_line_reader() {
    let mut r = std::io::Cursor::new(b"abc\nabcdefgh\nxy".to_vec());
    assert_eq!(read_line_capped(&mut r, 4).unwrap(), Some(b"abc".to_vec()));
    assert!(matches!(read_line_capped(&mut r, 4), Err(Ok(true))));
    assert_eq!(read_line_capped(&mut r, 4).unwrap(), Some(b"xy".to_vec()));
    assert_eq!(read_line_capped(&mut r, 4).unwrap(), None);
}

#[test]
fn a_missing_cli_is_an_error_not_a_panic() {
    let app = AppEntry {
        id: "nocraft".into(),
        name: "NoCraft".into(),
        command: std::env::temp_dir().join("no-such-craft-cli-for-tests"),
        args: vec!["mcp".into()],
    };
    let err = spawn(&app).unwrap_err();
    assert!(err.contains("could not start NoCraft"), "{err}");
}
