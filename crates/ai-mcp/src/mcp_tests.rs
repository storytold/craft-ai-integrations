use super::*;

fn req(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
}

fn gateway() -> McpState {
    let hub = crate::apps::Hub::with_connector(vec![], vec![], Box::new(|_| Err("none".into())));
    McpState::new().with_apps(Arc::new(hub))
}

#[test]
fn initialize_negotiates_a_published_revision() {
    let s = McpState::new();
    let r = s
        .handle(&req("initialize", json!({"protocolVersion": "2025-11-25"})))
        .unwrap();
    assert_eq!(r["result"]["protocolVersion"], "2025-11-25");
    assert!(r["result"]["capabilities"]["tools"].is_object());
    assert_eq!(r["result"]["serverInfo"]["name"], "craft-ai-mcp");
    for asked in [
        json!("2025-03-26"),
        json!("not-a-date"),
        json!(7),
        Value::Null,
    ] {
        let r = s
            .handle(&req("initialize", json!({"protocolVersion": asked})))
            .unwrap();
        assert_eq!(
            r["result"]["protocolVersion"], DEFAULT_PROTOCOL_VERSION,
            "{asked}"
        );
    }
}

#[test]
fn ping_notifications_and_unknown_methods() {
    let s = McpState::new();
    assert_eq!(
        s.handle(&req("ping", json!({}))).unwrap()["result"],
        json!({})
    );
    assert!(
        s.handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .is_none()
    );
    assert_eq!(
        s.handle(&req("nope", json!({}))).unwrap()["error"]["code"],
        -32601
    );
    assert_eq!(
        s.handle(&json!({"id": 1, "method": "ping"})).unwrap()["error"]["code"],
        -32600
    );
}

#[test]
fn the_gateway_tools_are_listed_only_with_a_hub() {
    let names = |s: &McpState| -> Vec<String> {
        s.handle(&req("tools/list", json!({}))).unwrap()["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(names(&McpState::new()).is_empty());
    let n = names(&gateway());
    for t in crate::apps::GATEWAY_TOOLS {
        assert!(n.iter().any(|x| x == t), "{t} missing");
    }
}

#[test]
fn tools_call_reaches_the_gateway_and_refuses_the_rest() {
    let s = gateway();
    let r = s
        .handle(&req(
            "tools/call",
            json!({"name": "craft_list_apps", "arguments": {}}),
        ))
        .unwrap();
    assert!(
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("apps")
    );
    for bad in [
        json!({"name": "generate-image"}),
        json!({"arguments": {}}),
        json!("x"),
    ] {
        let r = s.handle(&req("tools/call", bad.clone())).unwrap();
        assert!(r["error"].is_object(), "{bad}");
    }
}
