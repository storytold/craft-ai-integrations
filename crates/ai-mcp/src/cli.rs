//! The `craft-ai-mcp` command line, as a library function so other programs can ship the same
//! binary (the ArtCraft Launcher builds its own `craft-ai-mcp` with it).
//!
//! - `craft-ai-mcp --stdio`: MCP over stdin and stdout, the way desktop MCP clients start a local
//!   server. Offers the Craft AI tools and the gateway to the Craft apps ([`crate::apps`]);
//!   `--no-apps` leaves the gateway out. This is what the ArtCraft Launcher registers.
//! - `craft-ai-mcp` (no flag): streamable HTTP at `http://127.0.0.1:<CRAFT_MCP_PORT>/mcp` (default
//!   12345) for hosts that only speak HTTP. Offers the Craft AI tools only: the port has no caller
//!   authentication yet, so it does not drive the apps.
//!
//! Connections and keys come from the shared AI setup ([`AiClient::standard`]; `CRAFT_AI_DIR`
//! moves it). The bridge never grants consent itself; `CRAFT_MCP_AUTO_CONSENT=1` does, for trusted
//! local development only. Set `CRAFT_MCP_SKILLS_DIR` to also generate per-tool SKILL.md folders
//! for installation through Bionic's `skill.install` tool; the bridge never writes into Bionic's
//! skills folder itself.

use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use crate::McpState;
use craft_ai_client::AiClient;
use craft_ai_core::AppId;

const USAGE: &str = "usage: craft-ai-mcp [--stdio [--no-apps]]";

/// Run the bridge with `args` (without the program name), as the `craft-ai-mcp` binary does.
/// The ArtCraft Launcher ships its own `craft-ai-mcp` binary that calls this.
pub fn main(args: Vec<String>) -> ExitCode {
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("craft-ai-mcp: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let mut stdio = false;
    let mut apps = true;
    for a in &args {
        match a.as_str() {
            "--stdio" => stdio = true,
            "--no-apps" => apps = false,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unknown argument `{other}`\n{USAGE}")),
        }
    }

    let app = AppId::new("mcp").map_err(|e| e.to_string())?;
    let client = AiClient::standard(app).map_err(|e| format!("the shared AI setup: {e}"))?;
    let mut state = if env::var("CRAFT_MCP_AUTO_CONSENT").as_deref() == Ok("1") {
        eprintln!("craft-ai-mcp: CRAFT_MCP_AUTO_CONSENT=1, consent is granted without asking");
        McpState::with_policy(client, true, crate::mcp::DEFAULT_JOB_TIMEOUT)
    } else {
        McpState::new(client)
    };

    write_skills_if_asked();

    if stdio {
        if apps {
            let (entries, notes) = crate::apps::registry::discover();
            for n in &notes {
                eprintln!("craft-ai-mcp: {n}");
            }
            state = state.with_apps(Arc::new(crate::apps::Hub::new(entries, notes)));
        }
        let stdin = std::io::stdin().lock();
        let stdout = std::io::stdout();
        return crate::stdio::serve(&state, stdin, stdout).map_err(|e| e.to_string());
    }

    let port: u16 = match env::var("CRAFT_MCP_PORT") {
        Ok(p) => p
            .parse()
            .map_err(|_| format!("CRAFT_MCP_PORT `{p}` is not a port number"))?,
        Err(_) => 12345,
    };
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = crate::bind(addr).map_err(|e| format!("failed to bind {addr}: {e}"))?;
    println!("craft-ai-mcp listening on http://{addr}/mcp");
    crate::run(listener, state);
    Ok(())
}

/// Generate per-tool SKILL.md folders into `CRAFT_MCP_SKILLS_DIR` when it is set. Failures are
/// logged and do not stop the server (skills are optional). Bionic's own skills folder is refused:
/// writing there would overwrite installed SKILL.md files without `skill.install` validation.
fn write_skills_if_asked() {
    let Ok(dir_str) = env::var("CRAFT_MCP_SKILLS_DIR") else {
        return;
    };
    let dir = PathBuf::from(&dir_str);
    if crate::installer::inside_bionic_skills(&dir) {
        eprintln!(
            "refusing to generate skills into Bionic's skills directory ({}) - use a separate staging folder and install via skill.install",
            dir.display()
        );
        return;
    }
    match crate::installer::write_skills(&dir, &craft_ai_features::builtin()) {
        Ok(written) => eprintln!("wrote {} skill folders to {}", written.len(), dir.display()),
        Err(e) => eprintln!("failed to write skills to {}: {e}", dir.display()),
    }
}
