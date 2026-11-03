//! Entry point for the Craft AI MCP bridge.
//!
//! Listens on `CRAFT_MCP_PORT` (default 12345) and serves the Craft AI tool catalogue over
//! streamable HTTP at `/mcp`. The shared connections file is taken from `CRAFT_MCP_CONNECTIONS`;
//! when unset, a default location under the user's home directory is used. Secrets are read from an
//! in-memory store in v1, so remote tools report what is missing (the "prompt to install" signal)
//! until a real keychain is wired up.

use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use craft_ai_client::AiClient;
use craft_ai_core::AppId;
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;

fn main() {
    let port = env::var("CRAFT_MCP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(12345);
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().expect("bad port");

    let connections = default_connections_path();
    ensure_parent(&connections);
    let shared = SharedStore::new(connections.clone());
    let secrets = Arc::new(MemoryStore::new());
    let client = AiClient::new(AppId::new("mcp").unwrap(), shared, secrets);

    let state = craft_ai_mcp::McpState::new(client);

    match craft_ai_mcp::bind(addr) {
        Ok(listener) => {
            println!(
                "craft-ai-mcp listening on http://{addr}/mcp (connections: {})",
                connections.display()
            );
            craft_ai_mcp::run(listener, state);
        }
        Err(e) => {
            eprintln!("failed to bind {addr}: {e}");
            std::process::exit(1);
        }
    }
}

fn default_connections_path() -> PathBuf {
    env::var("CRAFT_MCP_CONNECTIONS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = env::var("HOME")
                .unwrap_or_else(|_| std::env::var("USERPROFILE").unwrap_or_default());
            PathBuf::from(home).join(".craft-ai/connections.json")
        })
}

fn ensure_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
}
