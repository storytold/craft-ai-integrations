//! Entry point for the Craft AI MCP bridge.
//!
//! Listens on `CRAFT_MCP_PORT` (default 12345) and serves the Craft AI tool catalogue over
//! streamable HTTP at `/mcp`. The shared connections file is taken from `CRAFT_MCP_CONNECTIONS`;
//! when unset, a default location under the user's home directory is used. Secrets are read from an
//! in-memory store in v1, so remote tools report what is missing (the "prompt to install" signal)
//! until a real keychain is wired up. Set `CRAFT_MCP_SKILLS_DIR` to also generate per-tool SKILL.md
//! folders for installation through Bionic's `skill.install` tool; the bridge never writes into
//! Bionic's skills folder itself.

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

    // Optionally generate per-tool SKILL.md folders for installation through Bionic's `skill.install`
    // tool. The bridge never writes into Bionic's skills folder itself; it only produces the folders
    // here so a user or script can install them. Set CRAFT_MCP_SKILLS_DIR to enable; failures are
    // logged but do not stop the server (skills are optional). If the requested destination is
    // Bionic's own skills directory, refuse - writing there would overwrite installed SKILL.md files
    // without skill.install validation or approval.
    if let Ok(dir_str) = env::var("CRAFT_MCP_SKILLS_DIR") {
        let dir = PathBuf::from(&dir_str);
        if is_bionic_skills_dir(&dir) {
            eprintln!("refusing to generate skills into Bionic's skills directory ({}) - use a separate staging folder and install via skill.install", dir.display());
        } else {
            match craft_ai_mcp::installer::write_skills(&dir, &craft_ai_features::builtin()) {
                Ok(written) => println!("wrote {} skill folders to {}", written.len(), dir.display()),
                Err(e) => eprintln!("failed to write skills to {}: {e}", dir.display()),
            }
        }
    }

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

/// True when `p` resolves inside Bionic's skills directory, where writing generated SKILL.md files
/// would overwrite installed skills without `skill.install` validation or approval.
fn is_bionic_skills_dir(p: &Path) -> bool {
    let canon = match p.canonicalize() {
        Ok(c) => c,
        Err(_) => return false,
    };
    let comps: Vec<_> = canon.components().collect();
    // True when the resolved path ends in `.lmstudio/skills`, independent of separator style.
    comps.len() >= 2
        && comps[comps.len() - 1].as_os_str().to_string_lossy().eq_ignore_ascii_case("skills")
        && comps[comps.len() - 2].as_os_str().to_string_lossy().eq_ignore_ascii_case(".lmstudio")
}
