//! The gateway's four MCP tools: their definitions for `tools/list` and their `tools/call`.

use std::time::Duration;

use serde_json::{Value, json};

use super::search::{DEFAULT_LIMIT, MAX_LIMIT};
use super::{
    CALL_TOOL, DEFAULT_CALL_TIMEOUT, DESCRIBE_TOOL, FIND_TOOLS, Hub, LIST_APPS, MAX_CALL_TIMEOUT,
};

/// The definitions `tools/list` adds for the gateway. Kept short: they are what every agent loads.
pub fn definitions() -> Vec<Value> {
    vec![
        json!({
            "name": LIST_APPS,
            "description": "List the Craft apps (PhotoCraft, FilmCraft, WordCraft, ...) you can use here. Start here.",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": {"readOnlyHint": true},
        }),
        json!({
            "name": FIND_TOOLS,
            "description": "Find an app's tools by a few words (\"blur\", \"new layer\", \"export pdf\"). Returns names and one-line summaries; get a tool's parameters with craft_describe_tool. Pass app to search one app (faster).",
            "inputSchema": {"type": "object", "properties": {
                "query": {"type": "string", "description": "A few words; empty lists the app's tools."},
                "app": {"type": "string", "description": "App id from craft_list_apps, e.g. photocraft."},
                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "default": DEFAULT_LIMIT},
            }, "required": ["query"]},
            "annotations": {"readOnlyHint": true},
        }),
        json!({
            "name": DESCRIBE_TOOL,
            "description": "One app tool's full definition, with its input schema.",
            "inputSchema": {"type": "object", "properties": {
                "app": {"type": "string"},
                "tool": {"type": "string"},
            }, "required": ["app", "tool"]},
            "annotations": {"readOnlyHint": true},
        }),
        json!({
            "name": CALL_TOOL,
            "description": "Run one app tool with its arguments. Returns the app's own result.",
            "inputSchema": {"type": "object", "properties": {
                "app": {"type": "string"},
                "tool": {"type": "string"},
                "arguments": {"type": "object", "description": "As the tool's input schema says."},
                "timeoutSeconds": {"type": "integer", "minimum": 1, "maximum": MAX_CALL_TIMEOUT.as_secs()},
            }, "required": ["app", "tool"]},
        }),
    ]
}

/// A tool result holding `v` as pretty JSON text.
fn text_result(v: &Value) -> Value {
    let text = serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string());
    json!({"content": [{"type": "text", "text": text}]})
}

/// A tool result saying what went wrong, so the agent can act on it.
pub fn error_result(message: &str) -> Value {
    json!({"content": [{"type": "text", "text": message}], "isError": true})
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("`{key}` is required and must be a string"))
}

/// Run gateway tool `name`. `None` when `name` is not one of the gateway's tools.
pub fn call(hub: &Hub, name: &str, args: &Value) -> Option<Value> {
    if !super::GATEWAY_TOOLS.contains(&name) {
        return None;
    }
    if let Err(e) = hub.check_switch() {
        return Some(error_result(&e));
    }
    let result = match name {
        LIST_APPS => Ok(text_result(&hub.list_apps())),
        FIND_TOOLS => find(hub, args),
        DESCRIBE_TOOL => str_arg(args, "app")
            .and_then(|app| Ok((app, str_arg(args, "tool")?)))
            .and_then(|(app, tool)| hub.describe_tool(app, tool))
            .map(|t| text_result(&t)),
        CALL_TOOL => call_tool(hub, args),
        _ => return None,
    };
    Some(result.unwrap_or_else(|e| error_result(&e)))
}

fn find(hub: &Hub, args: &Value) -> Result<Value, String> {
    let query = match args.get("query") {
        None | Some(Value::Null) => "",
        Some(Value::String(s)) => s.as_str(),
        Some(_) => return Err("`query` must be a string".into()),
    };
    let app = match args.get("app") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.trim().is_empty() => None,
        Some(Value::String(s)) => Some(s.as_str()),
        Some(_) => return Err("`app` must be a string".into()),
    };
    let limit = match args.get("limit") {
        None | Some(Value::Null) => DEFAULT_LIMIT,
        Some(v) => v
            .as_u64()
            .map(|n| usize::try_from(n).unwrap_or(MAX_LIMIT))
            .ok_or("`limit` must be a positive whole number")?,
    };
    hub.find_tools(query, app, limit).map(|v| text_result(&v))
}

fn call_tool(hub: &Hub, args: &Value) -> Result<Value, String> {
    let app = str_arg(args, "app")?;
    let tool = str_arg(args, "tool")?;
    let arguments = match args.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(v @ Value::Object(_)) => v.clone(),
        Some(_) => return Err("`arguments` must be an object".into()),
    };
    let timeout = match args.get("timeoutSeconds") {
        None | Some(Value::Null) => DEFAULT_CALL_TIMEOUT,
        Some(v) => {
            let secs = v
                .as_u64()
                .filter(|s| *s >= 1)
                .ok_or("`timeoutSeconds` must be a whole number of seconds, at least 1")?;
            Duration::from_secs(secs).min(MAX_CALL_TIMEOUT)
        }
    };
    hub.call_tool(app, tool, arguments, timeout)
}
