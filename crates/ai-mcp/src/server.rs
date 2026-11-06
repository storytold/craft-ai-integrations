//! Minimal raw-TCP streamable-HTTP transport for the Craft AI MCP bridge.
//!
//! No HTTP framework: we bind a socket, read one request per connection (headers + body), hand it
//! to [`McpState`](crate::mcp::McpState), and write back a plain-JSON response with `Connection:
//! close`. Plain JSON is spec-valid because MCP clients advertise `Accept: application/json,
//! text/event-stream` and accept either.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::mcp::McpState;

/// Maximum number of concurrent connections the server will serve at once. Overflowing peers get a
/// plain 503 rather than pinning unbounded threads.
const MAX_CONCURRENT_CONNECTIONS: usize = 16;

/// Hard cap on header bytes before we give up, so a malicious or buggy client cannot make us buffer
/// an unbounded amount of data while hunting for the end-of-headers marker.
const MAX_HEADER_BYTES: usize = 64 * 1024;

/// Total wall-clock budget for reading one request's headers. The per-read timeout alone is not
/// enough if a client trickles bytes just below it.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// Hard cap on the raw request body we will allocate, so a huge `Content-Length` cannot blow up
/// memory. Sized above the largest permitted inline image once base64-encoded (a 32 MiB raw
/// payload is ~43 MiB in base64) plus JSON overhead, while still bounding any single request.
const MAX_BODY_BYTES: usize = 96 * 1024 * 1024;

/// One parsed HTTP request (method, path, body).
#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    /// The `Content-Type` header value, if present.
    content_type: Option<String>,
    /// The `Origin` header value, if present.
    origin: Option<String>,
    body: Vec<u8>,
}

/// Bind the listener. The binary owns this and calls [`run`](Self::run) on it.
pub fn bind(addr: std::net::SocketAddr) -> Result<TcpListener, std::io::Error> {
    TcpListener::bind(addr)
}

/// Accept loop: one thread per connection so a slow job never blocks the next request. Concurrent
/// connections are capped at [`MAX_CONCURRENT_CONNECTIONS`]; overflow peers get a plain 503 instead
/// of pinning unbounded threads.
pub fn run(listener: TcpListener, state: McpState) {
    let state = Arc::new(state);
    let active = Arc::new(AtomicUsize::new(0));
    for result in listener.incoming() {
        let Ok(mut stream) = result else {
            continue;
        };
        // Reject overflow before spawning so we never exceed the cap.
        if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONCURRENT_CONNECTIONS {
            active.fetch_sub(1, Ordering::SeqCst);
            let _ = send_response(&mut stream, 503, "Service Unavailable", &Value::Null);
            continue;
        }
        let state = Arc::clone(&state);
        let counter = Arc::clone(&active);
        thread::spawn(move || {
            // A panic inside one connection must not take down the server.
            let _ = handle_connection(stream, &state);
            counter.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

/// Read one request, dispatch it, and write one response. Returns a human-readable note on failure.
fn handle_connection(mut stream: TcpStream, state: &McpState) -> Result<(), String> {
    // Bound how long we wait for a client so an idle or hung peer frees its thread.
    stream.set_read_timeout(Some(Duration::from_secs(60))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(30))).ok();

    let req = match read_request(&mut stream) {
        Ok(r) => r,
        Err(_e) => return send_response(&mut stream, 400, "Bad Request", &Value::Null),
    };

    // Only POST /mcp is a valid MCP endpoint; everything else gets a plain 405.
    if req.method != "POST" || !is_mcp_path(&req.path) {
        return send_response(&mut stream, 405, "Method Not Allowed", &Value::Null);
    }

    // Reject anything that is not JSON before it can reach a tool call: the MCP spec advertises
    // `application/json` and we refuse to guess at other encodings.
    if !is_json_content_type(req.content_type.as_deref()) {
        return send_response(&mut stream, 415, "Unsupported Media Type", &Value::Null);
    }

    // Reject any request carrying an `Origin` header — a browser-style cross-origin caller is not a
    // trusted local MCP client and must never reach our tools.
    if req.origin.is_some() {
        return send_response(&mut stream, 403, "Forbidden", &Value::Null);
    }

    let body = match serde_json::from_slice::<Value>(&req.body) {
        Ok(v) => v,
        Err(_e) => return send_response(&mut stream, 400, "Bad Request", &Value::Null),
    };

    // Notifications (no `id`) get no response per the JSON-RPC spec; close without writing.
    let Some(response) = state.handle(&body) else {
        return Ok(());
    };
    send_response(&mut stream, 200, "OK", &response)
}

/// True for the exact `/mcp` path, ignoring any query string (e.g. `/mcp?x=1`).
fn is_mcp_path(path: &str) -> bool {
    path.split('?').next().unwrap_or(path) == "/mcp"
}

/// True when `content_type` names JSON (case-insensitive, optional parameters allowed).
fn is_json_content_type(content_type: Option<&str>) -> bool {
    content_type
        .map(|ct| ct.split(';').next().unwrap_or(ct).trim())
        .is_some_and(|ct| ct.eq_ignore_ascii_case("application/json"))
}

/// Read a single HTTP request: headers up to the blank line, then the body per `Content-Length`.
fn read_request(stream: &mut TcpStream) -> Result<HttpRequest, String> {
    let buf = read_until_header_end(stream)?;
    let boundary =
        find_header_boundary(&buf).ok_or_else(|| "no end-of-headers found".to_string())?;

    let header_bytes = &buf[..boundary];
    let body_so_far = &buf[boundary..];

    // Parse the request line and headers.
    let text = String::from_utf8_lossy(header_bytes);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    let mut content_length: Option<u64> = None;
    let mut content_type: Option<String> = None;
    let mut origin: Option<String> = None;
    for line in lines {
        if let Some((key, value)) = split_header(line) {
            match key.to_ascii_lowercase().as_str() {
                "content-length" => content_length = value.trim().parse::<u64>().ok(),
                "content-type" => content_type = Some(value.trim().to_string()),
                "origin" => origin = Some(value.to_string()),
                _ => {}
            }
        }
    }

    // Reject a declared body larger than we are willing to allocate before we touch it.
    let declared = content_length.unwrap_or(0);
    if declared > MAX_BODY_BYTES as u64 {
        return Err(format!(
            "body too large: {} bytes exceeds the {} byte limit",
            declared, MAX_BODY_BYTES,
        ));
    }

    // Read the remainder of the body, if any.
    let have = body_so_far.len() as u64;
    let need = declared.saturating_sub(have);
    let mut body = body_so_far.to_vec();
    if need > 0 {
        let mut chunk = vec![0u8; need as usize];
        let mut got = 0usize;
        while got < need as usize {
            match stream.read(&mut chunk[got..]) {
                Ok(0) => return Err("connection closed mid-body".into()),
                Ok(n) => got += n,
                Err(e) => return Err(format!("read error: {e}")),
            }
        }
        body.extend_from_slice(&chunk);
    }

    Ok(HttpRequest {
        method,
        path,
        content_type,
        origin,
        body,
    })
}

/// Read bytes until the header block ends (`\r\n\r\n`). Returns everything seen so far. Bounded by
/// [`MAX_HEADER_BYTES`] and [`HEADER_READ_TIMEOUT`] so a client cannot make us buffer unbounded data
/// or pin a thread for an arbitrary time.
fn read_until_header_end(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let start = Instant::now();
    loop {
        if Instant::now().duration_since(start) > HEADER_READ_TIMEOUT {
            return Err("timed out waiting for request headers".into());
        }
        let mut chunk = vec![0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => return Err("connection closed before headers complete".into()),
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > MAX_HEADER_BYTES {
                    return Err("request headers too large".into());
                }
                if has_header_end(&buf) {
                    break;
                }
            }
            Err(e) => return Err(format!("read error: {e}")),
        }
    }
    Ok(buf)
}

fn has_header_end(buf: &[u8]) -> bool {
    buf.windows(4).any(|w| w == b"\r\n\r\n")
}

/// Index just past the first `\r\n\r\n` (where the body begins), or `None`.
fn find_header_boundary(buf: &[u8]) -> Option<usize> {
    for i in 0..buf.len().saturating_sub(3) {
        if buf[i] == b'\r' && buf[i + 1] == b'\n' && buf[i + 2] == b'\r' && buf[i + 3] == b'\n' {
            return Some(i + 4);
        }
    }
    None
}

/// Split a `key: value` header line into its parts.
fn split_header(line: &str) -> Option<(String, String)> {
    let (k, v) = line.split_once(':')?;
    Some((k.trim().to_string(), v.to_string()))
}

/// Write a minimal HTTP/1.1 response and close the connection.
fn send_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    body: &Value,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(body).unwrap_or_default();
    let mut resp = Vec::new();
    resp.extend_from_slice(format!("HTTP/1.1 {} {}\r\n", status, reason).as_bytes());
    resp.extend_from_slice(b"Content-Type: application/json\r\n");
    resp.extend_from_slice(format!("Content-Length: {}\r\n", bytes.len()).as_bytes());
    resp.extend_from_slice(b"Connection: close\r\n");
    resp.extend_from_slice(b"\r\n");
    resp.extend_from_slice(&bytes);
    stream
        .write_all(&resp)
        .map_err(|e| format!("write error: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_ai_client::AiClient;
    use craft_ai_core::AppId;
    use craft_ai_native::secrets::MemoryStore;
    use craft_ai_native::shared::SharedStore;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::tempdir;

    fn test_state() -> McpState {
        let dir = tempdir().unwrap();
        let shared = SharedStore::new(dir.path().join("connections.json"));
        let secrets = Arc::new(MemoryStore::new());
        McpState::with_policy(
            AiClient::new(AppId::new("mcp").unwrap(), shared, secrets),
            true,
            Duration::from_secs(5),
        )
    }

    /// Send a raw HTTP request and return the full response text.
    fn http_request(port: u16, body: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let req = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(req.as_bytes()).unwrap();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).unwrap();
        resp
    }

    /// Start a server on an ephemeral port and return the port. The listener is owned by the spawned
    /// thread, so it stays alive for the duration of the test.
    fn start_server(state: McpState) -> u16 {
        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = bind(addr).unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || run(listener, state));
        std::thread::sleep(Duration::from_millis(50));
        port
    }

    #[test]
    fn serve_initialize_over_tcp() {
        let state = test_state();
        let port = start_server(state);
        let resp = http_request(
            port,
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}",
        );
        assert!(resp.starts_with("HTTP/1.1 200 OK"));
        assert!(resp.contains("\"protocolVersion\""));
        assert!(resp.contains("2025-06-18"));
    }

    #[test]
    fn rejects_non_mcp_paths() {
        let state = test_state();
        let port = start_server(state);

        // POST /mcp is accepted.
        let resp = http_request(port, "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}");
        assert!(resp.starts_with("HTTP/1.1 200 OK"));

        // A GET to a non-mcp path is refused.
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let req = "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
        stream.write_all(req.as_bytes()).unwrap();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).unwrap();
        assert!(resp.starts_with("HTTP/1.1 405 Method Not Allowed"));
    }
}
