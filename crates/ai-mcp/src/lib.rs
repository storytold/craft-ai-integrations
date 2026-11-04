//! Bridge that exposes Craft AI tools to MCP clients (LM Studio / Bionic) over streamable HTTP at
//! `/mcp`. The shared catalogue ([`craft_ai_features::builtin`]) is the source of truth for which
//! tools exist and where they appear, and `tools/list` reflects exactly that catalogue.
//!
//! [`discovery`] is a helper that fetches the org's public repositories from GitHub so newly landed
//! apps can be noticed. It is not yet wired into `tools/list`, so a new app does not show up to MCP
//! hosts until it is added to the shared catalogue.

pub mod catalogue;
pub mod discovery;
pub mod router;

pub use catalogue::{ToolDescriptor, app_names, tools};
pub use discovery::{OrgRepo, fetch_from, fetch_org_repos};

pub mod installer;
pub mod mcp;
pub mod server;

pub use mcp::McpState;
pub use server::{bind, run};
