//! A tiny scripted HTTP server for tests. Binds to 127.0.0.1 on a free port, records every request,
//! and answers from a handler. No dependencies beyond std.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn body_json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub enum Step {
    Send(String),
    Wait(u64),
}

pub enum Reply {
    Json(u16, String),
    JsonWith(u16, Vec<(&'static str, String)>, String),
    Sse(Vec<Step>),
    /// A complete HTTP response, written as is (binary bodies, no Content-Length, and so on).
    Raw(Vec<u8>),
}

pub struct Server {
    /// Base URL a provider should use, ending in `/v1`.
    pub url: String,
    pub requests: Arc<Mutex<Vec<Recorded>>>,
    /// Set when a write to a client failed, meaning the client closed the connection.
    pub client_dropped: Arc<AtomicBool>,
}

impl Server {
    pub fn start(handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let dropped = Arc::new(AtomicBool::new(false));
        let handler = Arc::new(handler);
        let (reqs, drop_flag) = (Arc::clone(&requests), Arc::clone(&dropped));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (handler, reqs, drop_flag) = (
                    Arc::clone(&handler),
                    Arc::clone(&reqs),
                    Arc::clone(&drop_flag),
                );
                std::thread::spawn(move || serve(stream, &*handler, &reqs, &drop_flag));
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}/v1"),
            requests,
            client_dropped: dropped,
        }
    }

    pub fn recorded(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    /// A URL that nothing is listening on.
    pub fn dead_url() -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        format!("http://127.0.0.1:{port}/v1")
    }
}

/// Largest request head the test server reads.
const MAX_HEADER_BYTES: usize = 64 * 1024;

fn read_request(stream: &mut TcpStream) -> Option<Recorded> {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    // Only bytes not yet searched are scanned (with three back, for a delimiter split across
    // reads), and headers stop at a fixed size, so a client cannot make this quadratic.
    let mut searched = 0;
    let header_end = loop {
        if let Some(i) = buf[searched..].windows(4).position(|w| w == b"\r\n\r\n") {
            let end = searched + i;
            // A delimiter that starts past the limit does not make an oversized head acceptable.
            if end > MAX_HEADER_BYTES {
                return None;
            }
            break end;
        }
        searched = buf.len().saturating_sub(3);
        if buf.len() > MAX_HEADER_BYTES {
            return None;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_owned();
    let path = first.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let len = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < len {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Recorded {
        method,
        path,
        headers,
        body,
    })
}

fn serve(
    mut stream: TcpStream,
    handler: &(dyn Fn(&Recorded) -> Reply + Send + Sync),
    requests: &Mutex<Vec<Recorded>>,
    dropped: &AtomicBool,
) {
    let Some(req) = read_request(&mut stream) else {
        return;
    };
    requests.lock().unwrap().push(req.clone());
    match handler(&req) {
        Reply::Json(status, body) => write_json(&mut stream, status, &[], &body),
        Reply::JsonWith(status, extra, body) => write_json(&mut stream, status, &extra, &body),
        Reply::Raw(bytes) => {
            let _ = stream.write_all(&bytes);
            let _ = stream.flush();
        }
        Reply::Sse(steps) => {
            let head =
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            if stream.write_all(head.as_bytes()).is_err() {
                dropped.store(true, Ordering::SeqCst);
                return;
            }
            for step in steps {
                match step {
                    Step::Wait(ms) => std::thread::sleep(Duration::from_millis(ms)),
                    Step::Send(text) => {
                        if stream.write_all(text.as_bytes()).is_err() || stream.flush().is_err() {
                            dropped.store(true, Ordering::SeqCst);
                            return;
                        }
                    }
                }
            }
        }
    }
}

fn write_json(stream: &mut TcpStream, status: u16, extra: &[(&'static str, String)], body: &str) {
    let mut head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

/// One SSE data event.
pub fn sse(json: &str) -> Step {
    Step::Send(format!("data: {json}\n\n"))
}

/// A complete response with a binary body and a correct Content-Length.
pub fn http_bytes(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Reply {
    let mut out = format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(body);
    Reply::Raw(bytes)
}
