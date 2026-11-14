//! An MCP client for one app's own MCP server (`<app>-cli mcp`), spoken over its stdin and stdout:
//! one JSON-RPC message per line.
//!
//! [`Conn`] works on any reader and writer, so the tests drive it with an in-process fake server;
//! [`spawn`] connects it to a real child process. Every wait has a deadline and every line a size
//! cap, so a hung or chatty app cannot pin the gateway.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{
    Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError, sync_channel,
};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::registry::AppEntry;

/// Largest line read from an app. Screenshots come back as base64 inside one line.
pub const MAX_LINE: usize = 64 << 20;
/// How long an app may take to answer `initialize` (a cold start loads fonts and plug-ins).
pub const START_TIMEOUT: Duration = Duration::from_secs(60);
/// How long `tools/list` may take.
pub const LIST_TIMEOUT: Duration = Duration::from_secs(30);
/// Most `tools/list` pages read from one app.
const MAX_PAGES: usize = 64;
/// Messages the reader thread keeps for the gateway. When they are not picked up, the reader
/// waits, and so does the app writing them: a chatty app cannot fill the gateway's memory.
const QUEUE: usize = 256;
/// Lines waiting for the writer thread. A request writes one line and waits, so a full queue
/// means the app stopped reading.
const WRITE_QUEUE: usize = 64;
/// The notification an app sends when its tool list changed.
const TOOLS_CHANGED: &str = "notifications/tools/list_changed";
/// The MCP revision the gateway asks the apps for.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// What the reader thread hands over: a parsed message, or why the stream ended.
enum Incoming {
    Message(Value),
    TooLong,
    Closed(String),
}

/// A live connection to one app's MCP server.
pub struct Conn {
    /// Lines for the writer thread: a write the app does not take never blocks a request past
    /// its deadline.
    writer: SyncSender<Vec<u8>>,
    rx: Receiver<Incoming>,
    next_id: u64,
    child: Option<Child>,
    closed: Option<String>,
    /// The app said its tool list changed since [`Conn::take_tools_changed`] last asked.
    tools_changed: bool,
}

impl std::fmt::Debug for Conn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Conn")
            .field("next_id", &self.next_id)
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}

/// Read one line of at most `cap` bytes (without the newline). `Ok(None)` at end of stream;
/// `Err(true)` when the line was longer than `cap` (the rest of it is skipped).
fn read_line_capped(
    r: &mut impl BufRead,
    cap: usize,
) -> Result<Option<Vec<u8>>, Result<bool, String>> {
    let mut line = Vec::new();
    let mut too_long = false;
    loop {
        let buf = r.fill_buf().map_err(|e| Err(e.to_string()))?;
        if buf.is_empty() {
            return if line.is_empty() && !too_long {
                Ok(None)
            } else if too_long {
                Err(Ok(true))
            } else {
                Ok(Some(line))
            };
        }
        let (chunk, done) = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => (&buf[..i], Some(i + 1)),
            None => (buf, None),
        };
        if !too_long {
            if line.len().saturating_add(chunk.len()) > cap {
                too_long = true;
                line.clear();
            } else {
                line.extend_from_slice(chunk);
            }
        }
        let used = done.unwrap_or(buf.len());
        r.consume(used);
        if done.is_some() {
            return if too_long {
                Err(Ok(true))
            } else {
                Ok(Some(line))
            };
        }
    }
}

impl Conn {
    /// Connect to a server reading `writer` and writing `reader`. Starts a reader thread.
    pub fn new(reader: Box<dyn Read + Send>, writer: Box<dyn Write + Send>) -> Self {
        let (tx, rx) = sync_channel(QUEUE);
        let write_failed = tx.clone();
        std::thread::spawn(move || {
            let mut r = BufReader::new(reader);
            loop {
                let msg = match read_line_capped(&mut r, MAX_LINE) {
                    Ok(None) => Incoming::Closed("the app closed its MCP connection".into()),
                    Ok(Some(line)) => {
                        if line.iter().all(u8::is_ascii_whitespace) {
                            continue;
                        }
                        match serde_json::from_slice::<Value>(&line) {
                            Ok(v) => Incoming::Message(v),
                            // A stray log line on stdout is not fatal; skip it.
                            Err(_) => continue,
                        }
                    }
                    Err(Ok(_)) => Incoming::TooLong,
                    Err(Err(e)) => Incoming::Closed(format!("reading from the app failed: {e}")),
                };
                let end = matches!(msg, Incoming::Closed(_));
                if tx.send(msg).is_err() || end {
                    return;
                }
            }
        });
        let (wtx, wrx) = sync_channel::<Vec<u8>>(WRITE_QUEUE);
        std::thread::spawn(move || {
            let mut w = writer;
            for line in wrx {
                if let Err(e) = w.write_all(&line).and_then(|()| w.flush()) {
                    // Wake the request that is waiting: it would otherwise wait out its deadline.
                    // Waits for room in the queue (the request drains it); ends when the
                    // connection is dropped.
                    let _ = write_failed
                        .send(Incoming::Closed(format!("writing to the app failed: {e}")));
                    return;
                }
            }
        });
        Conn {
            writer: wtx,
            rx,
            next_id: 1,
            child: None,
            closed: None,
            tools_changed: false,
        }
    }

    /// Whether the app said its tool list changed since the last call (messages already queued
    /// are looked at too, without waiting).
    pub fn take_tools_changed(&mut self) -> bool {
        loop {
            match self.rx.try_recv() {
                Ok(Incoming::Message(v)) => {
                    if !matches!(self.answer_app_request(&v), Ok(true)) {
                        self.note(&v);
                    }
                }
                Ok(Incoming::TooLong) => {}
                Ok(Incoming::Closed(why)) => {
                    self.closed = Some(why);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.closed
                        .get_or_insert_with(|| "the app's MCP connection ended".into());
                    break;
                }
            }
        }
        std::mem::take(&mut self.tools_changed)
    }

    /// Answer a request the app sent (`Ok(true)`): `ping` with an empty result, as MCP asks,
    /// anything else (sampling, roots) as not supported. `Ok(false)` for any other message.
    fn answer_app_request(&mut self, v: &Value) -> Result<bool, String> {
        let (Some(rid), Some(method)) = (v.get("id"), v.get("method")) else {
            return Ok(false);
        };
        let reply = if method.as_str() == Some("ping") {
            json!({"jsonrpc": "2.0", "id": rid, "result": {}})
        } else {
            json!({"jsonrpc": "2.0", "id": rid,
                "error": {"code": -32601, "message": "not supported by the Craft gateway"}})
        };
        self.send(&reply)?;
        Ok(true)
    }

    /// Remember a notification the gateway cares about.
    fn note(&mut self, v: &Value) {
        if v.get("id").is_none() && v.get("method").and_then(Value::as_str) == Some(TOOLS_CHANGED) {
            self.tools_changed = true;
        }
    }

    /// Why this connection can no longer be used, if it can't.
    pub fn closed(&self) -> Option<&str> {
        self.closed.as_deref()
    }

    fn send(&mut self, msg: &Value) -> Result<(), String> {
        let mut line = serde_json::to_vec(msg).map_err(|e| e.to_string())?;
        line.push(b'\n');
        // Never wait to queue a line: an app that stops reading fills the queue, and the
        // connection is closed then rather than holding lines without end.
        self.writer.try_send(line).map_err(|e| {
            let why = match e {
                TrySendError::Full(_) => {
                    "writing to the app failed: it stopped reading".to_string()
                }
                TrySendError::Disconnected(_) => "writing to the app failed".to_string(),
            };
            self.closed = Some(why.clone());
            why
        })
    }

    /// Send a notification (no reply).
    pub fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// Send a request and wait up to `timeout` for its reply's `result`. An error reply, a closed
    /// stream or the deadline is an `Err`. After a deadline or a lost reply the connection is
    /// marked closed: its state is unknown, so the caller starts a new one.
    pub fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        if let Some(why) = &self.closed {
            return Err(why.clone());
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let msg = match self.rx.recv_timeout(left) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => {
                    let why = format!(
                        "the app did not answer `{method}` within {}s",
                        timeout.as_secs()
                    );
                    self.closed = Some(why.clone());
                    return Err(why);
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let why = "the app's MCP connection ended".to_string();
                    self.closed = Some(why.clone());
                    return Err(why);
                }
            };
            let v = match msg {
                Incoming::Message(v) => v,
                Incoming::TooLong => {
                    let why = format!("the app sent a message larger than {MAX_LINE} bytes");
                    self.closed = Some(why.clone());
                    return Err(why);
                }
                Incoming::Closed(why) => {
                    self.closed = Some(why.clone());
                    return Err(why);
                }
            };
            if self.answer_app_request(&v)? {
                continue;
            }
            if v.get("id").and_then(Value::as_u64) != Some(id) {
                self.note(&v);
                continue; // a notification, or a late reply to an earlier request
            }
            if let Some(err) = v.get("error") {
                let text = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("the app returned an error");
                return Err(text.to_string());
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// The MCP handshake.
    pub fn initialize(&mut self) -> Result<Value, String> {
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "craft-ai-mcp", "version": env!("CARGO_PKG_VERSION")},
        });
        let result = self.request("initialize", params, START_TIMEOUT)?;
        self.notify("notifications/initialized", json!({}))?;
        Ok(result)
    }

    /// Every tool the app offers (all pages).
    pub fn list_tools(&mut self) -> Result<Vec<Value>, String> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let page = self.request("tools/list", params, LIST_TIMEOUT)?;
            if let Some(list) = page.get("tools").and_then(Value::as_array) {
                tools.extend(
                    list.iter()
                        .filter(|t| t.get("name").and_then(Value::as_str).is_some())
                        .cloned(),
                );
            }
            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            if cursor.is_none() {
                return Ok(tools);
            }
        }
        // A partial list would be cached as complete, and calls to the rest refused.
        Err(format!(
            "the app lists its tools in more than {MAX_PAGES} pages"
        ))
    }

    /// Call one tool. The app's result (its `content`, `isError` and so on) is returned as is.
    pub fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.request(
            "tools/call",
            json!({"name": name, "arguments": arguments}),
            timeout,
        )
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Start `app`'s MCP server and complete the handshake.
pub fn spawn(app: &AppEntry) -> Result<Conn, String> {
    let mut cmd = Command::new(&app.command);
    cmd.args(&app.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// No console window for a CLI started from a GUI client.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| {
        format!(
            "could not start {} ({}): {e}",
            app.name,
            app.command.display()
        )
    })?;
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        return Err(format!("could not connect to {}'s MCP server", app.name));
    };
    let mut conn = Conn::new(Box::new(stdout), Box::new(stdin));
    conn.child = Some(child);
    conn.initialize()
        .map_err(|e| format!("{} did not start its MCP server: {e}", app.name))?;
    Ok(conn)
}

#[cfg(test)]
#[path = "child_tests.rs"]
pub(crate) mod tests;
