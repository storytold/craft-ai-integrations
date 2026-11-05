//! Bridge that exposes Craft AI tools to MCP clients (LM Studio / Bionic) over streamable HTTP at
//! `/mcp`. The shared catalogue ([`craft_ai_features::builtin`]) is the source of truth for which
//! tools exist and where they appear, and `tools/list` reflects exactly that catalogue.
//!
//! [`discovery`] is a helper that fetches the org's public repositories from GitHub so newly landed
//! apps can be noticed. It is not yet wired into `tools/list`, so a new app does not show up to MCP
//! hosts until it is added to the shared catalogue.

/// Catalogue: normalizes the shared feature list into flat tool descriptors used by `tools/list`.
pub mod catalogue;
/// Discovery: fetches the storytold org's public repositories so newly landed Craft apps can be noticed.
pub mod discovery;
/// Router: routes one MCP `tools/call` through the shared client into a Craft AI job.
pub mod router;

pub use catalogue::{ToolDescriptor, app_names, tools};
pub use discovery::{OrgRepo, fetch_from, fetch_org_repos};

/// Installer: merges this bridge's entry into Bionic's `ng-mcp.json` and generates per-tool skill folders.
pub mod installer;
/// Mcp: JSON-RPC 2.0 handling for the Craft AI MCP bridge (initialize, tools/list, ping, tools/call).
pub mod mcp;
/// Server: minimal raw-TCP streamable-HTTP transport feeding parsed requests to [`mcp::McpState`].
pub mod server;

pub use mcp::McpState;
pub use server::{bind, run};
