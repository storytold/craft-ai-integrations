//! Discovery against a scripted localhost stand-in for the GitHub API (no real network).

use std::io::{Read, Write};
use std::net::TcpListener;
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
            let mut buf = Vec::new();
            loop {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        if buf.len() >= 4 && &buf[buf.len() - 4..] == b"\r\n\r\n" {
                            break;
                        } else {
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
