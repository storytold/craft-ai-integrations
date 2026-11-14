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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};
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
/// Whether the person still lets agents use the Craft apps ([`registry::switched_on`]).
type Switch = dyn Fn() -> bool + Send + Sync;

/// What an agent is told while the launcher's switch is off.
pub const SWITCHED_OFF: &str = "AI agents are turned off for the Craft apps. Turn on ArtCraft Launcher › Settings › AI agents › Let AI agents use Craft apps, then try again.";
/// One app's tools, or why they could not be read.
type ToolList = Result<Arc<Vec<Value>>, String>;

/// One app's connection and its tool list, under separate locks: a call holds the connection
/// for as long as it runs (up to [`MAX_CALL_TIMEOUT`]), and listing the apps, searching and
/// describing a tool must not wait for it. Lock order: `conn`, then `tools`. `conn` is only
/// taken through [`Hub::hold`].
#[derive(Default)]
struct Slot {
    conn: Mutex<Option<Conn>>,
    tools: Mutex<Option<Arc<Vec<Value>>>>,
    /// Agents were turned off while someone held `conn`: [`Hub::check_switch`] could not stop the
    /// app's server then, so the holder does it when it lets go.
    stop: AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The lock if nobody holds it; `None` while a call does.
fn try_lock<T>(m: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    match m.try_lock() {
        Ok(g) => Some(g),
        Err(TryLockError::Poisoned(e)) => Some(e.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

impl Slot {
    /// Stop the app's server (its process is killed when the connection drops) and forget its
    /// tools. The caller holds `conn`.
    fn close(&self, conn: &mut Option<Conn>) {
        *conn = None;
        *lock(&self.tools) = None;
    }
}

/// The apps and their live connections.
pub struct Hub {
    apps: Vec<AppEntry>,
    notes: Vec<String>,
    slots: Vec<Slot>,
    connect: Box<Connector>,
    switch: Box<Switch>,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub")
            .field("apps", &self.apps)
            .finish_non_exhaustive()
    }
}

impl Hub {
    /// A hub that starts each app's real CLI while the launcher's switch is on.
    pub fn new(apps: Vec<AppEntry>, notes: Vec<String>) -> Self {
        Self::with_connector(apps, notes, Box::new(child::spawn))
            .with_switch(Box::new(registry::switched_on))
    }

    /// Ask `switch` before every tool call (tests; [`Hub::new`] asks the launcher's app list).
    pub fn with_switch(mut self, switch: Box<Switch>) -> Self {
        self.switch = switch;
        self
    }

    /// `Err` while the person has turned agents off. Every app server this gateway started is
    /// stopped then, so nothing keeps running on the apps after the switch goes off. One whose
    /// connection is held (a call, or a moment's look from listing or searching) is stopped by
    /// its holder when it lets go ([`Self::hold`]), without this answer waiting.
    pub fn check_switch(&self) -> Result<(), String> {
        if (self.switch)() {
            return Ok(());
        }
        for slot in &self.slots {
            // Flag first, then try: a holder that lets go after this sees the flag.
            slot.stop.store(true, Ordering::SeqCst);
            if let Some(mut conn) = try_lock(&slot.conn) {
                slot.close(&mut conn);
                slot.stop.store(false, Ordering::SeqCst);
            }
        }
        Err(SWITCHED_OFF.into())
    }

    /// Run `f` holding `slot`'s connection: waiting for it with `wait`, otherwise only if it is
    /// free (`None` while someone else holds it). After letting it go, stop the app's server if
    /// agents were turned off meanwhile and [`Self::check_switch`] left that to this holder.
    fn hold<R>(
        &self,
        slot: &Slot,
        wait: bool,
        f: impl FnOnce(&mut Option<Conn>) -> R,
    ) -> Option<R> {
        let mut conn = if wait {
            lock(&slot.conn)
        } else {
            try_lock(&slot.conn)?
        };
        let out = f(&mut conn);
        drop(conn);
        // If someone else holds it now, they see the flag when they let go.
        if slot.stop.load(Ordering::SeqCst)
            && let Some(mut conn) = try_lock(&slot.conn)
        {
            slot.stop.store(false, Ordering::SeqCst);
            if !(self.switch)() {
                slot.close(&mut conn);
            }
        }
        Some(out)
    }

    /// A hub with its own way to connect to an app (tests).
    pub fn with_connector(
        apps: Vec<AppEntry>,
        notes: Vec<String>,
        connect: Box<Connector>,
    ) -> Self {
        let slots = apps.iter().map(|_| Slot::default()).collect();
        Hub {
            apps,
            notes,
            slots,
            connect,
            switch: Box::new(|| true),
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

    /// Run `f` on `app`'s connection and its tool list, starting it (or replacing a dead one) and
    /// reading the list first when it isn't known, so the list is never missing while a caller
    /// holds the app: listing, searching and describing never wait for a call. The switch is
    /// asked again once the connection is this caller's: a call that waited for the app while
    /// agents were turned off never starts. One that was running when they were turned off
    /// finishes, then the app's server is stopped.
    fn with_conn<R>(
        &self,
        i: usize,
        f: impl FnOnce(&mut Conn, &Arc<Vec<Value>>) -> Result<R, String>,
    ) -> Result<R, String> {
        let (Some(app), Some(slot)) = (self.apps.get(i), self.slots.get(i)) else {
            return Err("no such app".into());
        };
        let run = |conn: &mut Option<Conn>| {
            if !(self.switch)() {
                slot.close(conn);
                return Err(SWITCHED_OFF.into());
            }
            if conn.as_mut().is_some_and(Conn::take_tools_changed) {
                *lock(&slot.tools) = None;
            }
            if conn.as_ref().is_none_or(|c| c.closed().is_some()) {
                // A new process may offer other tools than the one it replaces.
                slot.close(conn);
                *conn = Some((self.connect)(app)?);
            }
            let Some(c) = conn.as_mut() else {
                return Err(format!("{} is not connected", app.name));
            };
            let known = lock(&slot.tools).clone();
            let list = match known {
                Some(list) => list,
                None => {
                    let list = Arc::new(c.list_tools()?);
                    *lock(&slot.tools) = Some(Arc::clone(&list));
                    list
                }
            };
            let out = f(c, &list);
            if !(self.switch)() {
                slot.close(conn);
            }
            out
        };
        self.hold(slot, true, run)
            .unwrap_or_else(|| Err(format!("{} is not connected", app.name)))
    }

    /// `app`'s tools, read once and again after the app says they changed. While a call holds
    /// the app, the list read before it is used: the app can't announce a change mid-call that
    /// matters to a caller waiting on it anyway.
    fn tools(&self, i: usize) -> Result<Arc<Vec<Value>>, String> {
        let Some(slot) = self.slots.get(i) else {
            return Err("no such app".into());
        };
        self.hold(slot, false, |conn| {
            let changed = conn.as_mut().is_some_and(Conn::take_tools_changed);
            // An app that changed its tools, or that stopped, gets its list read again: the next
            // use starts it anew, and a restarted app may offer other tools.
            if changed || conn.as_ref().is_some_and(|c| c.closed().is_some()) {
                *lock(&slot.tools) = None;
            }
        });
        if let Some(t) = lock(&slot.tools).as_ref() {
            return Ok(Arc::clone(t));
        }
        self.with_conn(i, |_, list| Ok(Arc::clone(list)))
    }

    /// `craft_list_apps`.
    pub fn list_apps(&self) -> Value {
        let apps: Vec<Value> = self
            .apps
            .iter()
            .zip(&self.slots)
            .map(|(a, s)| {
                let mut o = Map::new();
                o.insert("app".into(), json!(a.id));
                o.insert("name".into(), json!(a.name));
                // A call holds the connection: the app runs, and is busy with it.
                let running = self.hold(s, false, |c| {
                    c.as_ref().is_some_and(|c| c.closed().is_none())
                });
                let started = match running {
                    Some(started) => started,
                    None => {
                        o.insert("busy".into(), json!(true));
                        true
                    }
                };
                o.insert("started".into(), json!(started));
                if let Some(t) = lock(&s.tools).as_ref() {
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
        let offers = |tools: &[Value]| {
            if tools
                .iter()
                .any(|t| t.get("name").and_then(Value::as_str) == Some(tool))
            {
                Ok(())
            } else {
                Err(format!(
                    "{app} has no tool `{tool}`; use {FIND_TOOLS} to look for one"
                ))
            }
        };
        offers(&self.tools(i)?)?;
        // Asked again: the app may have been restarted while this call waited for it.
        self.with_conn(i, |conn, list| {
            offers(list)?;
            conn.call_tool(tool, arguments, timeout)
        })
    }
}

pub mod tools;

#[cfg(test)]
#[path = "hub_tests.rs"]
mod hub_tests;
