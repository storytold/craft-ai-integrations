//! The `craft-ai-mcp` command line, as a library function so other programs can ship the same
//! binary (the ArtCraft Launcher builds its own `craft-ai-mcp` with it).
//!
//! `craft-ai-mcp --stdio` serves MCP on stdin and stdout, the way desktop MCP clients start a
//! local server: the gateway to the Craft apps listed in `ArtCraft/mcp/apps.json`
//! ([`crate::apps::registry`]). There is no network listener: the client owns the process, so
//! nothing else on the computer can reach it.

use std::process::ExitCode;
use std::sync::Arc;

use crate::McpState;

const USAGE: &str = "usage: craft-ai-mcp --stdio";

/// Run with `args` (without the program name), as the `craft-ai-mcp` binary does.
pub fn main(args: Vec<String>) -> ExitCode {
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("craft-ai-mcp: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    match args {
        [a] if a == "--stdio" => {}
        [a] if a == "-h" || a == "--help" => {
            println!("{USAGE}");
            return Ok(());
        }
        [] => return Err(format!("nothing to do\n{USAGE}")),
        _ => return Err(format!("unknown arguments `{}`\n{USAGE}", args.join(" "))),
    }
    let (entries, notes) = crate::apps::registry::discover();
    for n in &notes {
        eprintln!("craft-ai-mcp: {n}");
    }
    let state = McpState::new().with_apps(Arc::new(crate::apps::Hub::new(entries, notes)));
    crate::stdio::serve(&state, std::io::stdin().lock(), std::io::stdout())
        .map_err(|e| e.to_string())
}
