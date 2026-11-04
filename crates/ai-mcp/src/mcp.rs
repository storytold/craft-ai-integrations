//! JSON-RPC 2.0 handling for the Craft AI MCP bridge.
//!
//! This is protocol-only: it maps MCP `initialize` / `tools/list` / `ping` / `tools/call` onto
//! the shared catalogue and the [`router`](crate::router). It knows nothing about sockets or HTTP;
//! the transport in [`server`](crate::server) feeds it parsed requests and hands back responses.

use std::time::Duration;

use craft_ai_client::AiClient;
use craft_ai_core::InputRole;
use serde_json::{Map, Value};

/// Default wall-clock budget for a routed job. Long enough for video generation, short enough that
/// a hung provider does not pin a thread forever. Overridable by the binary (see `main.rs`).
pub const DEFAULT_JOB_TIMEOUT: Duration = Duration::from_secs(900);

/// A published revision the bridge falls back to when the client does not request one. The
/// previously hardcoded "2025-03-07" was not a published revision and LM Studio rejected it.
pub const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// True for a well-formed `YYYY-MM-DD` protocol-revision string.
fn is_protocol_version(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() == 10 && b[4] == b'-' && b[7] == b'-'
        && (0..b.len()).all(|i| i != 4 && i != 7 && b[i].is_ascii_digit())
}

/// Everything the transport needs to answer one MCP request.
#[derive(Debug)]
pub struct McpState {
    client: AiClient,
    /// When true, any consent a remote destination would ask for is granted up front (trusted local
    /// host). The router still never asks on local connections regardless of this flag.
    auto_consent: bool,
    timeout: Duration,
}

impl McpState {
    pub fn new(client: AiClient) -> Self {
        Self {
            client,
            auto_consent: true,
            timeout: DEFAULT_JOB_TIMEOUT,
        }
    }

    /// Build a state with an explicit job budget and consent policy (used by tests / the binary).
    pub fn with_policy(client: AiClient, auto_consent: bool, timeout: Duration) -> Self {
        Self {
            client,
            auto_consent,
            timeout,
        }
    }

    /// Answer one JSON-RPC request. Returns `None` for notifications (no `id`) — the spec says
    /// these get no response. Otherwise returns a complete response object (success or error).
    pub fn handle(&self, request: &Value) -> Option<Value> {
        // Reject requests that are not JSON-RPC 2.0 before dispatching anything.
        if request.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0") {
            return Some(error_response(
                request.get("id").cloned().unwrap_or(Value::Null),
                -32600,
                "Invalid Request".to_string(),
            ));
        }
        let has_id = request.get("id").is_some();
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");

        // Notifications (no `id`) get no response.
        if !has_id {
            return None;
        }

        match method {
            "initialize" => Some(self.ok(id, self.initialize(request))),
            "tools/list" => Some(self.ok(id, self.tools_list())),
            "ping" => Some(self.ok(id, Ok(Value::Object(Map::new())))),
            "tools/call" => Some(self.ok(id, self.tools_call(request))),
            _ => Some(error_response(id, -32601, format!("unknown method `{method}`"))),
        }
    }

    fn ok(&self, id: Value, result: Result<Value, String>) -> Value {
        match result {
            Ok(v) => success_response(id, v),
            Err(e) => error_response(id, -32601, e),
        }
    }

    /// `initialize` — negotiate the protocol version and advertise that we expose tools.
    fn initialize(&self, request: &Value) -> Result<Value, String> {
        // Negotiate: echo the client's requested revision when it is a well-formed date; otherwise
        // fall back to a published revision the bridge supports. (The old hardcoded "2025-03-07"
        // was not a published revision and LM Studio rejected it.)
        let version = match request
            .get("params")
            .and_then(|p| p.get("protocolVersion"))
            .and_then(|v| v.as_str())
        {
            Some(v) if is_protocol_version(v) => v.to_string(),
            _ => DEFAULT_PROTOCOL_VERSION.to_string(),
        };

        let mut tools_cap = Map::new();
        tools_cap.insert("listChanged".into(), Value::Bool(false));
        let mut capabilities = Map::new();
        capabilities.insert("tools".into(), Value::Object(tools_cap));

        let mut server_info = Map::new();
        server_info.insert("name".into(), Value::String("craft-ai-mcp".into()));
        server_info.insert(
            "version".into(),
            Value::String(env!("CARGO_PKG_VERSION").to_string()),
        );

        let mut out = Map::new();
        out.insert("protocolVersion".into(), Value::String(version));
        out.insert("capabilities".into(), Value::Object(capabilities));
        out.insert("serverInfo".into(), Value::Object(server_info));
        Ok(Value::Object(out))
    }

    /// `tools/list` — every built-in feature as an MCP tool, with a JSON Schema for its inputs.
    fn tools_list(&self) -> Result<Value, String> {
        let mut tool_values = Vec::new();
        for f in craft_ai_features::builtin() {
            let (props, required) = input_schema(&f);
            let mut schema = Map::new();
            schema.insert("type".into(), Value::String("object".into()));
            schema.insert("properties".into(), Value::Object(props));
            if !required.is_empty() {
                schema.insert(
                    "required".into(),
                    Value::Array(required.into_iter().map(Value::String).collect()),
                );
            }

            let mut tool = Map::new();
            tool.insert("name".into(), Value::String(f.id.clone()));
            tool.insert(
                "description".into(),
                Value::String(format!("{} — {}", f.title, f.summary)),
            );
            tool.insert("inputSchema".into(), Value::Object(schema));
            tool_values.push(Value::Object(tool));
        }

        let mut out = Map::new();
        out.insert("tools".into(), Value::Array(tool_values));
        Ok(Value::Object(out))
    }

    /// `tools/call` — route one named tool through the client. Unknown tool names are JSON-RPC
    /// errors; known-tool execution outcomes become MCP tool results with `isError` so the host
    /// surfaces them plainly (this is also the "what is missing" signal). Inline image outputs are
    /// returned as `image` content (base64 + MIME type); text stays as `text` content.
    fn tools_call(&self, request: &Value) -> Result<Value, String> {
        // MCP puts tool parameters under `params`. Accept a top-level fallback for any
        // non-conforming client that nests name/arguments directly on the request.
        let params = match request.get("params") {
            Some(p) if p.is_object() => p.clone(),
            _ => request.clone(),
        };
        let empty_args = Value::Object(Map::new());
        let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
        let tool_args = params.get("arguments").cloned().unwrap_or(empty_args);

        // Unknown tool names are a client error (JSON-RPC -32601). Known-tool execution outcomes
        // become MCP tool results with `isError` so the host can surface them plainly.
        if !craft_ai_features::builtin()
            .iter()
            .any(|f| f.id == name)
        {
            return Err(format!("unknown tool `{name}`"));
        }

        let (is_error, items): (bool, Vec<Value>) = match crate::router::route_tool(
            &self.client,
            name,
            tool_args,
            self.auto_consent,
            self.timeout,
        ) {
            crate::router::ToolCallResult::Succeeded { text, media } => {
                let mut items: Vec<Value> = Vec::new();
                for line in &text {
                    items.push(Self::text_item(line));
                }
                for m in &media {
                    items.push(Self::image_item(&m.mime_type, &m.data));
                }
                (false, items)
            }
            crate::router::ToolCallResult::Unavailable { reason } => {
                (true, vec![Self::text_item(&reason)])
            }
            crate::router::ToolCallResult::NeedsConsent { disclosure } => {
                (true, vec![Self::text_item(&disclosure)])
            }
            crate::router::ToolCallResult::Failed { message } => {
                (true, vec![Self::text_item(&message)])
            }
        };

        let mut out = Map::new();
        out.insert("content".into(), Value::Array(items));
        out.insert("isError".into(), Value::Bool(is_error));
        Ok(Value::Object(out))
    }

    /// A single `text` content item.
    fn text_item(text: &str) -> Value {
        let mut m = Map::new();
        m.insert("type".into(), Value::String("text".into()));
        m.insert("text".into(), Value::String(text.to_string()));
        Value::Object(m)
    }

    /// A single `image` content item (base64-encoded bytes + original MIME type).
    fn image_item(media_type: &str, data: &[u8]) -> Value {
        let mut m = Map::new();
        m.insert("type".into(), Value::String("image".into()));
        m.insert(
            "data".into(),
            Value::String(craft_ai_core::payload::base64_encode(data)),
        );
        m.insert("mimeType".into(), Value::String(media_type.to_string()));
        Value::Object(m)
    }
}

/// Build the `properties` + `required` of a tool's input schema from its declared inputs.
fn input_schema(f: &craft_ai_features::Feature) -> (Map<String, Value>, Vec<String>) {
    let mut props = Map::new();
    let mut required = Vec::new();
    for input in &f.inputs {
        match input {
            InputRole::Prompt => {
                props.insert("prompt".into(), str_prop());
                required.push("prompt".to_string());
            }
            InputRole::Source => {
                props.insert("source_path".into(), str_prop());
                props.insert("source_base64".into(), str_prop());
            }
            InputRole::Mask => {
                props.insert("mask_path".into(), str_prop());
                props.insert("mask_base64".into(), str_prop());
            }
            // Reference / Context / System are app-specific (clicks, etc.) and not expressible through
            // plain JSON args in v1; the router rejects them with a clear message.
            _ => {}
        }
    }
    (props, required)
}

fn str_prop() -> Value {
    let mut m = Map::new();
    m.insert("type".into(), Value::String("string".into()));
    Value::Object(m)
}

/// A JSON-RPC success envelope.
fn success_response(id: Value, result: Value) -> Value {
    let mut out = Map::new();
    out.insert("jsonrpc".into(), Value::String("2.0".into()));
    out.insert("id".into(), id);
    out.insert("result".into(), result);
    Value::Object(out)
}

/// A JSON-RPC error envelope.
fn error_response(id: Value, code: i32, message: String) -> Value {
    let mut err = Map::new();
    err.insert("code".into(), Value::Number(code.into()));
    err.insert("message".into(), Value::String(message));
    let mut out = Map::new();
    out.insert("jsonrpc".into(), Value::String("2.0".into()));
    out.insert("id".into(), id);
    out.insert("error".into(), Value::Object(err));
    Value::Object(out)
}

#[cfg(test)]
mod tests {
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
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "name": "generate-image",
            "arguments": { "prompt": "a cat" }
        });
        let resp = state.handle(&req).unwrap();
        // Known tool with no model bound -> error tool result, not a JSON-RPC error.
        assert_eq!(resp["result"]["isError"], true);
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
}
