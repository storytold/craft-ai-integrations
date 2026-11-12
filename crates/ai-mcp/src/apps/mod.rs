//! The gateway to the Craft apps: one MCP entry for every installed app.
//!
//! Each app already has its own MCP server (`<app>-cli mcp`). Listing all of their tools at once
//! would fill much of an agent's context, so the gateway offers four tools instead
//! ([`GATEWAY_TOOLS`]): list the apps, find tools by a few words, describe one tool, call one tool.
//! An app's MCP server is started the first time an agent needs that app, and its tool list is
//! kept for the life of the gateway.

pub mod child;
pub mod registry;
pub mod search;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Map, Value, json};

use child::Conn;
use registry::AppEntry;

/// The gateway's tool names.
pub const LIST_APPS: &str = "craft_list_apps";
pub const FIND_TOOLS: &str = "craft_find_tools";
pub const DESCRIBE_TOOL: &str = "craft_describe_tool";
pub const CALL_TOOL: &str = "craft_call_tool";
pub const GATEWAY_TOOLS: [&str; 4] = [LIST_APPS, FIND_TOOLS, DESCRIBE_TOOL, CALL_TOOL];

/// How long one app tool may run when the caller does not say.
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(600);
/// Longest wait a caller may ask for.
pub const MAX_CALL_TIMEOUT: Duration = Duration::from_secs(3600);

type Connector = dyn Fn(&AppEntry) -> Result<Conn, String> + Send + Sync;
/// One app's tools, or why they could not be read.
type ToolList = Result<Arc<Vec<Value>>, String>;

#[derive(Default)]
struct Slot {
    conn: Option<Conn>,
    tools: Option<Arc<Vec<Value>>>,
}

/// The apps and their live connections.
pub struct Hub {
    apps: Vec<AppEntry>,
    notes: Vec<String>,
    slots: Vec<Mutex<Slot>>,
    connect: Box<Connector>,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub")
            .field("apps", &self.apps)
            .finish_non_exhaustive()
    }
}

impl Hub {
    /// A hub that starts each app's real CLI.
    pub fn new(apps: Vec<AppEntry>, notes: Vec<String>) -> Self {
        Self::with_connector(apps, notes, Box::new(child::spawn))
    }

    /// A hub with its own way to connect to an app (tests).
    pub fn with_connector(
        apps: Vec<AppEntry>,
        notes: Vec<String>,
        connect: Box<Connector>,
    ) -> Self {
        let slots = apps.iter().map(|_| Mutex::new(Slot::default())).collect();
        Hub {
            apps,
            notes,
            slots,
            connect,
        }
    }

    fn index(&self, app: &str) -> Result<usize, String> {
        let id = app.trim().to_lowercase();
        self.apps.iter().position(|a| a.id == id).ok_or_else(|| {
            let known: Vec<&str> = self.apps.iter().map(|a| a.id.as_str()).collect();
            if known.is_empty() {
                format!("no Craft app is available to agents (asked for `{app}`); turn agents on in the ArtCraft Launcher")
            } else {
                format!("no app `{app}`; available: {}", known.join(", "))
            }
        })
    }

    /// Run `f` on `app`'s connection, starting it (or replacing a dead one) first.
    fn with_conn<R>(
        &self,
        i: usize,
        f: impl FnOnce(&mut Conn, &mut Option<Arc<Vec<Value>>>) -> Result<R, String>,
    ) -> Result<R, String> {
        let (Some(app), Some(slot)) = (self.apps.get(i), self.slots.get(i)) else {
            return Err("no such app".into());
        };
        let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.conn.as_ref().is_none_or(|c| c.closed().is_some()) {
            slot.conn = None;
            slot.conn = Some((self.connect)(app)?);
        }
        let Slot { conn, tools } = &mut *slot;
        let Some(conn) = conn.as_mut() else {
            return Err(format!("{} is not connected", app.name));
        };
        f(conn, tools)
    }

    /// `app`'s tools, read once and again after the app says they changed.
    fn tools(&self, i: usize) -> Result<Arc<Vec<Value>>, String> {
        if let Some(slot) = self.slots.get(i) {
            let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
            if slot.conn.as_mut().is_some_and(Conn::take_tools_changed) {
                slot.tools = None;
            }
            if let Some(t) = &slot.tools {
                return Ok(Arc::clone(t));
            }
        }
        self.with_conn(i, |conn, cache| {
            let list = Arc::new(conn.list_tools()?);
            *cache = Some(Arc::clone(&list));
            Ok(list)
        })
    }

    /// `craft_list_apps`.
    pub fn list_apps(&self) -> Value {
        let apps: Vec<Value> = self
            .apps
            .iter()
            .zip(&self.slots)
            .map(|(a, s)| {
                let s = s.lock().unwrap_or_else(PoisonError::into_inner);
                let mut o = Map::new();
                o.insert("app".into(), json!(a.id));
                o.insert("name".into(), json!(a.name));
                o.insert(
                    "started".into(),
                    json!(s.conn.as_ref().is_some_and(|c| c.closed().is_none())),
                );
                if let Some(t) = &s.tools {
                    o.insert("tools".into(), json!(t.len()));
                }
                Value::Object(o)
            })
            .collect();
        let mut out = json!({ "apps": apps });
        if !self.notes.is_empty() {
            out["notes"] = json!(self.notes);
        }
        out
    }

    /// `craft_find_tools`.
    pub fn find_tools(
        &self,
        query: &str,
        app: Option<&str>,
        limit: usize,
    ) -> Result<Value, String> {
        let which: Vec<usize> = match app {
            Some(a) => vec![self.index(a)?],
            None => (0..self.apps.len()).collect(),
        };
        // Every app is started at once, so one that hangs in its start does not hold up the
        // others; the answer waits at most for the slowest one's own deadline.
        let results: Vec<(usize, ToolList)> = std::thread::scope(|s| {
            let handles: Vec<_> = which
                .iter()
                .map(|&i| (i, s.spawn(move || self.tools(i))))
                .collect();
            handles
                .into_iter()
                .map(|(i, h)| {
                    let r = h
                        .join()
                        .unwrap_or_else(|_| Err("reading the app's tools failed".to_string()));
                    (i, r)
                })
                .collect()
        });
        let mut lists = Vec::new();
        let mut unavailable = Vec::new();
        for (i, r) in results {
            let Some(entry) = self.apps.get(i) else {
                continue;
            };
            match r {
                Ok(t) => lists.push((entry.id.as_str(), t)),
                Err(e) if app.is_some() => return Err(e),
                Err(e) => unavailable.push(json!({"app": entry.id, "error": e})),
            }
        }
        let hits = search::find(query, lists.iter().map(|(a, t)| (*a, t.as_slice())), limit);
        let hits: Vec<Value> = hits
            .into_iter()
            .map(|h| json!({"app": h.app, "tool": h.tool, "summary": h.summary}))
            .collect();
        let empty = hits.is_empty();
        let mut out = json!({ "tools": hits });
        if empty {
            // Some apps (PhotoCraft among them) run most of their actions as engine commands
            // behind one or two tools; point at those instead of returning nothing.
            let catalogs: Vec<String> = lists
                .iter()
                .flat_map(|(a, t)| {
                    t.iter()
                        .filter_map(|x| x.get("name").and_then(Value::as_str))
                        .filter(|n| n.contains("command"))
                        .map(move |n| format!("{a}/{n}"))
                })
                .take(search::MAX_LIMIT)
                .collect();
            if !catalogs.is_empty() {
                out["hint"] = json!(format!(
                    "No tool matched. These tools list or run an app's own commands; describe one and search its commands: {}",
                    catalogs.join(", ")
                ));
            }
        }
        if !unavailable.is_empty() {
            out["unavailable"] = Value::Array(unavailable);
        }
        Ok(out)
    }

    /// `craft_describe_tool`: the tool's full definition, input schema included.
    pub fn describe_tool(&self, app: &str, tool: &str) -> Result<Value, String> {
        let i = self.index(app)?;
        let tools = self.tools(i)?;
        tools
            .iter()
            .find(|t| t.get("name").and_then(Value::as_str) == Some(tool))
            .cloned()
            .ok_or_else(|| format!("{app} has no tool `{tool}`; use {FIND_TOOLS} to look for one"))
    }

    /// `craft_call_tool`: the app's own tool result, passed through unchanged.
    pub fn call_tool(
        &self,
        app: &str,
        tool: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let i = self.index(app)?;
        let tools = self.tools(i)?;
        if !tools
            .iter()
            .any(|t| t.get("name").and_then(Value::as_str) == Some(tool))
        {
            return Err(format!(
                "{app} has no tool `{tool}`; use {FIND_TOOLS} to look for one"
            ));
        }
        self.with_conn(i, |conn, _| conn.call_tool(tool, arguments, timeout))
    }
}

pub mod tools;

#[cfg(test)]
#[path = "hub_tests.rs"]
mod hub_tests;
