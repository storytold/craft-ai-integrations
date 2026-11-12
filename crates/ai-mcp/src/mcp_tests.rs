use super::*;
use craft_ai_core::AppId;
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;
use serde_json::json;
use std::sync::Arc;
use tempfile::tempdir;

/// A client with no saved connections and an empty key store: every remote tool reports what is
/// missing, which is exactly the "prompt to install" path we want to exercise.
fn test_client() -> AiClient {
    let dir = tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let secrets = Arc::new(MemoryStore::new());
    AiClient::new(AppId::new("mcp").unwrap(), shared, secrets)
}

#[test]
fn initialize_advertises_tools() {
    let state = McpState::new(test_client());
    let req = init_req("initialize");
    let resp = state.handle(&req).unwrap();
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(
        resp["result"]["protocolVersion"].as_str().unwrap(),
        "2025-06-18"
    );
    assert!(resp["result"]["capabilities"]["tools"].is_object());
}

#[test]
fn initialize_echoes_a_supported_requested_version() {
    let state = McpState::new(test_client());
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2025-11-25" },
    });
    let resp = state.handle(&req).unwrap();
    assert_eq!(
        resp["result"]["protocolVersion"].as_str().unwrap(),
        "2025-11-25"
    );
}

#[test]
fn initialize_falls_back_for_an_unsupported_requested_version() {
    let state = McpState::new(test_client());
    // A well-formed but unsupported revision must fall back to the default, not be echoed.
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2025-03-26" },
    });
    let resp = state.handle(&req).unwrap();
    assert_eq!(
        resp["result"]["protocolVersion"].as_str().unwrap(),
        DEFAULT_PROTOCOL_VERSION
    );
}

#[test]
fn initialize_rejects_a_malformed_version_with_fallback() {
    let state = McpState::new(test_client());
    // A malformed (not `YYYY-MM-DD`) revision must fall back to the default.
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "not-a-date" },
    });
    let resp = state.handle(&req).unwrap();
    assert_eq!(
        resp["result"]["protocolVersion"].as_str().unwrap(),
        DEFAULT_PROTOCOL_VERSION
    );
}

#[test]
fn is_protocol_version_accepts_well_formed_dates() {
    assert!(is_protocol_version("2025-06-18"));
    assert!(is_protocol_version("2025-11-25"));
    assert!(is_protocol_version("2024-11-05"));
}

#[test]
fn is_protocol_version_rejects_malformed_dates() {
    assert!(!is_protocol_version("")); // empty
    assert!(!is_protocol_version("2025-06-1")); // too short
    assert!(!is_protocol_version("abcd-ef-gh")); // non-digit positions
    assert!(!is_protocol_version("2025-06-18x")); // too long
}

#[test]
fn ping_returns_empty_result() {
    let state = McpState::new(test_client());
    let resp = state.handle(&init_req("ping")).unwrap();
    assert_eq!(resp["result"], Value::Object(Map::new()));
}

#[test]
fn tools_list_returns_every_builtin() {
    let state = McpState::new(test_client());
    let resp = state.handle(&init_req("tools/list")).unwrap();
    let tools = resp["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), craft_ai_features::builtin().len());
    // Every tool has a name, description and an object schema.
    for t in tools {
        assert!(t["name"].is_string());
        assert!(t["description"].is_string());
        assert_eq!(t["inputSchema"]["type"], "object");
    }
}

#[test]
fn tools_call_reports_what_is_missing() {
    let state = McpState::new(test_client());
    let req = call_req("generate-image", json!({"prompt": "a cat"}));
    let resp = state.handle(&req).unwrap();
    // No model is bound, so the router must surface a plain reason as an error tool result.
    assert_eq!(resp["result"]["isError"], true);
    assert!(!resp["result"]["content"].as_array().unwrap().is_empty());
}

#[test]
fn tools_call_unknown_tool_is_an_error() {
    let state = McpState::new(test_client());
    let req = call_req("not-a-tool", json!({}));
    let resp = state.handle(&req).unwrap();
    assert!(resp["error"].is_object());
}

#[test]
fn tools_call_accepts_top_level_params_for_leniency() {
    let state = McpState::new(test_client());
    // Non-conforming client: name/arguments at the top level instead of under `params`.
    // We still route to the named tool rather than treating it as unknown.
    let lenient_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "name": "generate-image",
        "arguments": { "prompt": "a cat" }
    });
    let lenient_resp = state.handle(&lenient_req).unwrap();

    // The same call in the standard wire format (params under `params`) must produce an
    // identical result — proving the lenient path routes to the named tool rather than any
    // arbitrary error.
    let std_req = call_req("generate-image", json!({ "prompt": "a cat" }));
    let std_resp = state.handle(&std_req).unwrap();

    assert_eq!(lenient_resp["result"], std_resp["result"]);
}

/// Tiny helper so tests read like the wire format.
fn init_req(method: &str) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": method })
}

/// A `tools/call` request in the standard MCP wire format (params under `params`).
fn call_req(name: &str, args: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": args }
    })
}

#[test]
fn new_never_grants_consent_itself() {
    // A person approves in the app or launcher; the bridge's default must not do it for them.
    assert!(!McpState::new(test_client()).auto_consent);
}

#[test]
fn the_gateway_tools_are_listed_only_with_a_hub() {
    let names = |s: &McpState| -> Vec<String> {
        s.handle(&init_req("tools/list")).unwrap()["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    };
    let plain = McpState::new(test_client());
    assert!(!names(&plain).iter().any(|n| n.starts_with("craft_")));
    let hub = crate::apps::Hub::with_connector(vec![], vec![], Box::new(|_| Err("none".into())));
    let with = McpState::new(test_client()).with_apps(Arc::new(hub));
    let n = names(&with);
    for t in crate::apps::GATEWAY_TOOLS {
        assert!(n.iter().any(|x| x == t), "{t} missing");
    }
    let resp = with
        .handle(&call_req("craft_list_apps", json!({})))
        .unwrap();
    assert!(
        resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("apps")
    );
}

#[test]
fn a_picture_input_requires_its_path_or_its_base64() {
    let state = McpState::new(test_client());
    let resp = state.handle(&init_req("tools/list")).unwrap();
    let tools = resp["result"]["tools"].as_array().unwrap();
    let fill = tools
        .iter()
        .find(|t| t["name"] == "generative-fill")
        .unwrap();
    let all_of = fill["inputSchema"]["allOf"].as_array().unwrap();
    assert!(all_of.contains(
        &json!({"anyOf": [{"required": ["source_path"]}, {"required": ["source_base64"]}]})
    ));
    assert!(
        all_of.contains(
            &json!({"anyOf": [{"required": ["mask_path"]}, {"required": ["mask_base64"]}]})
        )
    );
    let gen_img = tools
        .iter()
        .find(|t| t["name"] == "generate-image")
        .unwrap();
    assert!(gen_img["inputSchema"].get("allOf").is_none());
}

#[test]
fn outputs_become_image_audio_or_resource_content() {
    let img = McpState::media_item(0, "image/png", b"x");
    assert_eq!(img["type"], "image");
    let audio = McpState::media_item(1, "audio/wav", b"x");
    assert_eq!(audio["type"], "audio");
    let video = McpState::media_item(2, "video/mp4", b"abc");
    assert_eq!(video["type"], "resource");
    assert_eq!(video["resource"]["mimeType"], "video/mp4");
    assert_eq!(
        video["resource"]["blob"],
        craft_ai_core::payload::base64_encode(b"abc")
    );
}
