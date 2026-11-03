//! Bridge that exposes Craft AI tools to MCP clients (LM Studio / Bionic), with dynamic
//! discovery of new apps and tools across the storytold org's public repositories.
//!
//! The shared catalogue ([`craft_ai_features::builtin`]) is the source of truth for which tools
//! exist and where they appear. [`discovery`] watches the org's public repos to surface apps that
//! have landed but are not yet listed in the catalogue, so a new app (or tool it offers) shows up
//! without editing this crate.

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
