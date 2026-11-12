//! `craft-ai-mcp`: one MCP server through which an agent (Claude, Cursor, LM Studio, ...) reaches
//! every installed Craft app.
//!
//! Each Craft app already serves MCP from its own CLI (`<app>-cli mcp`). Registering every app
//! separately means one client entry per app, and each lists all of its tools up front. The
//! gateway is one entry with four tools ([`apps::tools`]): list the apps, find tools by a few
//! words, describe one tool, call one tool. An app's MCP server is started the first time an agent
//! needs it ([`apps`]).
//!
//! The ArtCraft Launcher turns it on (Settings › AI agents), writes the app list it reads
//! ([`apps::registry`]) and registers `craft-ai-mcp --stdio` in the MCP clients ([`cli`],
//! [`stdio`]).

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

/// The gateway to every installed Craft app's own MCP server.
pub mod apps;
/// The `craft-ai-mcp` command line, callable from other binaries (the launcher ships one).
pub mod cli;
/// MCP's JSON-RPC methods (initialize, ping, tools/list, tools/call).
pub mod mcp;
/// MCP over stdin and stdout, how desktop MCP clients start a local server.
pub mod stdio;

pub use mcp::McpState;
