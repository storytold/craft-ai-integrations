//! JSON-RPC 2.0 for the gateway: MCP `initialize`, `ping`, `tools/list` and `tools/call`, mapped
//! onto the gateway's tools ([`crate::apps::tools`]). It knows nothing about the transport; the
//! stdio loop in [`crate::stdio`] feeds it parsed requests and writes back its responses.

use std::sync::Arc;

use serde_json::{Map, Value, json};

use crate::apps::Hub;

/// Published MCP revisions the gateway answers with. A client's requested revision is echoed only
/// when it is one of these; otherwise [`DEFAULT_PROTOCOL_VERSION`].
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 2] = ["2025-06-18", "2025-11-25"];

/// The revision used when a client asks for none, or for one not in the list.
pub const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// Everything needed to answer one MCP request.
#[derive(Debug, Default)]
pub struct McpState {
    apps: Option<Arc<Hub>>,
}

impl McpState {
    /// A state with no tools; add the gateway with [`Self::with_apps`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Offer the gateway to the Craft apps.
    #[must_use]
    pub fn with_apps(mut self, hub: Arc<Hub>) -> Self {
        self.apps = Some(hub);
        self
    }

    /// Answer one JSON-RPC request. `None` for a notification (no `id`), which gets no response.
    pub fn handle(&self, request: &Value) -> Option<Value> {
        let id = request.get("id").cloned();
        if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(error_response(
                id.unwrap_or(Value::Null),
                -32600,
                "Invalid Request",
            ));
        }
        let id = id?;
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let result = match method {
            "initialize" => Ok(initialize(request)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(self.tools_list()),
            "tools/call" => self.tools_call(request),
            _ => {
                return Some(error_response(
                    id,
                    -32601,
                    &format!("unknown method `{method}`"),
                ));
            }
        };
        Some(match result {
            Ok(v) => json!({"jsonrpc": "2.0", "id": id, "result": v}),
            Err(e) => error_response(id, -32602, &e),
        })
    }

    fn tools_list(&self) -> Value {
        let tools = if self.apps.is_some() {
            crate::apps::tools::definitions()
        } else {
            Vec::new()
        };
        json!({ "tools": tools })
    }

    fn tools_call(&self, request: &Value) -> Result<Value, String> {
        let params = request
            .get("params")
            .filter(|p| p.is_object())
            .ok_or("`params` must be an object")?;
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or("`params.name` must be a string")?;
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        self.apps
            .as_ref()
            .and_then(|hub| crate::apps::tools::call(hub, name, &args))
            .ok_or_else(|| format!("unknown tool `{name}`"))
    }
}

/// True for a published revision this gateway answers with.
fn supported_protocol_version(v: &str) -> bool {
    SUPPORTED_PROTOCOL_VERSIONS.contains(&v)
}

/// `initialize`: the negotiated revision, the tools capability and who we are.
fn initialize(request: &Value) -> Value {
    let version = request
        .get("params")
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str)
        .filter(|v| supported_protocol_version(v))
        .unwrap_or(DEFAULT_PROTOCOL_VERSION);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "craft-ai-mcp", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Craft apps (PhotoCraft, FilmCraft, WordCraft, ...). Start with craft_list_apps, find a tool with craft_find_tools, read its parameters with craft_describe_tool, run it with craft_call_tool.",
    })
}

fn error_response(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
