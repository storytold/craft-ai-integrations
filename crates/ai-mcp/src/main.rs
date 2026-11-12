//! Entry point for the Craft AI MCP bridge.
//!
//! Listens on `CRAFT_MCP_PORT` (default 12345) and serves the Craft AI tool catalogue over
//! streamable HTTP at `/mcp`. By default it reads the user's saved connections from the canonical
//! shared store and keys from the environment then the OS keychain (`AiClient::standard`). Set
//! `CRAFT_MCP_CONNECTIONS` to point at a custom connections file for testing/dev. Consent is
//! fail-closed: the bridge never grants it on its own - a person approves in the ArtCraft Launcher
//! (see craft-ai-integrations#164). Set `CRAFT_MCP_SKILLS_DIR` to also generate per-tool SKILL.md
//! folders for installation through Bionic's `skill.install` tool; the bridge never writes into
//! Bionic's skills folder itself.

use std::env;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use craft_ai_client::AiClient;
use craft_ai_core::AppId;
use craft_ai_native::secrets::{EnvStore, KeychainStore, LayeredStore};
use craft_ai_native::shared::SharedStore;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = env::var("CRAFT_MCP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(12345);
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse()?;

    // Default to the canonical shared store + env/keychain secrets (AiClient::standard). When
    // CRAFT_MCP_CONNECTIONS is set, honor it as an opt-in override for testing/dev while still
    // reading keys from env then the OS keychain.
    let client = if let Ok(p) = env::var("CRAFT_MCP_CONNECTIONS") {
        // Named PathBuf so the same value can be borrowed for ensure_parent and then moved
        // into SharedStore (borrow ends when ensure_parent returns).
        let path = PathBuf::from(&p);
        ensure_parent(&path);
        let shared = SharedStore::new(path);
        let secrets = Arc::new(LayeredStore::new(vec![
            Box::new(EnvStore::new()),
            Box::new(KeychainStore::new()),
        ]));
        AiClient::new(AppId::new("mcp")?, shared, secrets)
    } else {
        // Inner ? unwraps AppId::new; outer ? unwraps AiClient::standard. No trailing semicolon
        // so this branch yields an AiClient like the other.
        AiClient::standard(AppId::new("mcp")?)?
    };

    // Fail closed: the bridge never grants consent on its own. A person approves in the launcher
    // (see craft-ai-integrations#164); the router still never asks on local connections regardless.
    let state =
        craft_ai_mcp::McpState::with_policy(client, false, craft_ai_mcp::DEFAULT_JOB_TIMEOUT);

    // Optionally generate per-tool SKILL.md folders for installation through Bionic's `skill.install`
    // tool. The bridge never writes into Bionic's skills folder itself; it only produces the folders
    // here so a user or script can install them. Set CRAFT_MCP_SKILLS_DIR to enable; failures are
    // logged but do not stop the server (skills are optional). If the requested destination is
    // Bionic's own skills directory, refuse - writing there would overwrite installed SKILL.md files
    // without skill.install validation or approval.
    if let Ok(dir_str) = env::var("CRAFT_MCP_SKILLS_DIR") {
        let dir = PathBuf::from(&dir_str);
        if is_bionic_skills_dir(&dir) {
            eprintln!(
                "refusing to generate skills into Bionic's skills directory ({}) - use a separate staging folder and install via skill.install",
                dir.display()
            );
        } else {
            match craft_ai_mcp::installer::write_skills(&dir, &craft_ai_features::builtin()) {
                Ok(written) => {
                    println!("wrote {} skill folders to {}", written.len(), dir.display())
                }
                Err(e) => eprintln!("failed to write skills to {}: {e}", dir.display()),
            }
        }
    }

    match craft_ai_mcp::bind(addr) {
        Ok(listener) => {
            println!("craft-ai-mcp listening on http://{addr}/mcp");
            craft_ai_mcp::run(listener, state);
        }
        Err(e) => {
            eprintln!("failed to bind {addr}: {e}");
            std::process::exit(1);
        }
    }
    Ok(())
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
        && comps[comps.len() - 1]
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("skills")
        && comps[comps.len() - 2]
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(".lmstudio")
}
