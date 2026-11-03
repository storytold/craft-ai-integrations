//! Minimal raw-TCP streamable-HTTP transport for the Craft AI MCP bridge.
//!
//! No HTTP framework: we bind a socket, read one request per connection (headers + body), hand it
//! to [`McpState`](crate::mcp::McpState), and write back a plain-JSON response with `Connection:
//! close`. Plain JSON is spec-valid because MCP clients advertise `Accept: application/json,
//! text/event-stream` and accept either.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::mcp::McpState;

/// One parsed HTTP request (method, path, body).
#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

/// Bind the listener. The binary owns this and calls [`run`](Self::run) on it.
pub fn bind(addr: std::net::SocketAddr) -> Result<TcpListener, std::io::Error> {
    TcpListener::bind(addr)
}

/// Accept loop: one thread per connection so a slow job never blocks the next request.
pub fn run(listener: TcpListener, state: McpState) {
    let state = Arc::new(state);
    for stream in listener.incoming() {
        if let Ok(stream) = stream {
            let state = Arc::clone(&state);
            thread::spawn(move || {
                // A panic inside one connection must not take down the server.
                let _ = handle_connection(stream, &state);
            });
        }
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
    if req.method != "POST" || !req.path.starts_with("/mcp") {
        return send_response(&mut stream, 405, "Method Not Allowed", &Value::Null);
    }

    let body = match serde_json::from_slice::<Value>(&req.body) {
        Ok(v) => v,
        Err(_e) => return send_response(&mut stream, 400, "Bad Request", &Value::Null),
    };

    let response = state.handle(&body);
    send_response(&mut stream, 200, "OK", &response)
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
    for line in lines {
        if let Some((key, value)) = split_header(line) {
            if key.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<u64>().ok();
            }
        }
    }

    // Read the remainder of the body, if any.
    let have = body_so_far.len() as u64;
    let need = content_length.unwrap_or(0).saturating_sub(have);
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

    Ok(HttpRequest { method, path, body })
}

/// Read bytes until the header block ends (`\r\n\r\n`). Returns everything seen so far.
fn read_until_header_end(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    loop {
        let mut chunk = vec![0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => return Err("connection closed before headers complete".into()),
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
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
    buf.windows(4).any(|w| w == [b'\r', b'\n', b'\r', b'\n'])
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
        assert!(resp.contains("2025-03-07"));
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
