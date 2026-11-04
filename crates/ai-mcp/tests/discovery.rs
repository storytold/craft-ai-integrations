//! Discovery against a scripted localhost stand-in for the GitHub API (no real network).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Spawns a one-shot HTTP server on 127.0.0.1 that answers the next request with `json`.
fn serve_json_once(json: &str) -> String {
    let body = json.to_string();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            // Read the request into a bounded chunk and append it to `buf` until the header block
            // ends (`\r\n\r\n`) or the connection closes/errors — never respond before the full
            // request has been read.
            let mut buf = Vec::new();
            loop {
                let mut chunk = vec![0u8; 4096];
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if buf.len() >= 4 && &buf[buf.len() - 4..] == b"\r\n\r\n" {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
            // Give the client a beat to drain the response before we tear down, so the OS
            // closes with a clean FIN rather than a RST (avoids flaky 10054 on Windows).
            thread::sleep(Duration::from_millis(100));
        }
    });
    format!("http://127.0.0.1:{port}/repos")
}

/// Spawns an HTTP server on 127.0.0.1 that answers successive requests with the given JSON bodies
/// in order. Every page except the last advertises a `Link` header pointing back at the same
/// endpoint, so a paginating client must make one request per page and stop after the final one.
fn serve_json_sequence(pages: Vec<String>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let base_url = format!("http://127.0.0.1:{port}/repos");
    let queue = Arc::new(Mutex::new(pages));
    thread::spawn(move || {
        loop {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                // Read the request until its headers end before responding.
                let mut buf = Vec::new();
                loop {
                    let mut chunk = vec![0u8; 4096];
                    match stream.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if buf.len() >= 4 && &buf[buf.len() - 4..] == b"\r\n\r\n" {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let mut q = queue.lock().unwrap();
                let Some(body) = q.pop() else { continue; };
                let has_next = !q.is_empty();
                let mut resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
                    body.len(),
                );
                if has_next {
                    resp.push_str(&format!("Link: <{}>; rel=\"next\"; type=\"application/json\"\r\n", base_url));
                }
                resp.push_str("Connection: close\r\n\r\n");
                resp.push_str(&body);
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        }
    });
    format!("http://127.0.0.1:{port}/repos")
}

#[test]
fn discovery_filters_craft_apps_and_detects_new_ones() {
    let json = r#"[
      {"name":"photocraft","description":"An open-source, photo editor written by scratch in pure Rust"},
      {"name":"craft-anything","description":"understand how any software works, and craft your own."},
      {"name":"unreal-demo","description":"A sample Unreal Engine project"},
      {"name":"old-craft","description":"An old Craft app","archived":true}
    ]"#;
    let url = serve_json_once(json);
    let repos = craft_ai_mcp::discovery::fetch_from(&url).unwrap();
    assert_eq!(repos.len(), 4);

    let apps = craft_ai_mcp::discovery::craft_apps(&repos);
    assert!(
        apps.iter().any(|a| a == "photocraft"),
        "expected photocraft"
    );
    assert!(
        apps.iter().any(|a| a == "craft-anything"),
        "expected craft-anything"
    );
    assert!(
        !apps.iter().any(|a| a == "unreal-demo"),
        "unreal-demo should be filtered out"
    );
    assert!(
        !apps.iter().any(|a| a == "old-craft"),
        "archived old-craft should be excluded"
    );

    let known = craft_ai_mcp::catalogue::app_names();
    let unknown = craft_ai_mcp::discovery::unknown_apps(&known, &apps);
    assert!(
        unknown.iter().any(|a| a == "craft-anything"),
        "craft-anything is new to the catalogue"
    );
}

#[test]
fn discovery_follows_pagination_link_headers() {
    let page1 = r#"[{"name":"photocraft","description":"photo editor"},{"name":"craft-anything","description":"understand software"}]"#;
    let page2 = r#"[{"name":"unreal-demo","description":"Unreal project"},{"name":"old-craft","description":"old app","archived":true}]"#;
    // Two pages: the first advertises a next link, the second does not. fetch_from must combine
    // both (4 repos), not just return the first page's 2.
    let url = serve_json_sequence(vec![page1.to_string(), page2.to_string()]);
    let repos = craft_ai_mcp::discovery::fetch_from(&url).unwrap();
    assert_eq!(repos.len(), 4, "expected both pages combined");

    let names: Vec<String> = repos.iter().map(|r| r.name.clone()).collect();
    assert!(names.contains(&"photocraft".to_string()), "missing page-1 entry");
    assert!(names.contains(&"unreal-demo".to_string()), "missing page-2 entry (pagination)");
}
