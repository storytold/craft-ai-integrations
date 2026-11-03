//! Normalizes the built-in feature catalogue into flat tool descriptors an MCP server can list.

use std::collections::HashSet;

use craft_ai_features::{AppStatus, Feature};
use serde::Serialize;

/// One app that uses a tool, with its maturity in the shared catalogue.
#[derive(Clone, Debug, Serialize)]
pub struct AppRef {
    pub app: String,
    pub tool: String,
    pub status: AppStatus,
}

/// A flat description of one user-facing AI tool, ready to be advertised as an MCP tool.
#[derive(Clone, Debug, Serialize)]
pub struct ToolDescriptor {
    /// Stable id; also the role id apps bind to inherit the launcher's choice.
    pub id: String,
    pub title: String,
    pub summary: String,
    /// The capability a provider must advertise to run it (e.g. `image.segment`).
    pub capability: String,
    /// Where the tool appears and how mature each appearance is.
    pub apps: Vec<AppRef>,
}

/// Every built-in feature as an MCP-listable descriptor, in launcher order.
pub fn tools() -> Vec<ToolDescriptor> {
    craft_ai_features::builtin()
        .iter()
        .map(to_descriptor)
        .collect()
}

/// Every app named in the shared catalogue, sorted and de-duplicated.
pub fn app_names() -> Vec<String> {
    let mut set = HashSet::new();
    for f in craft_ai_features::builtin() {
        for u in &f.uses {
            set.insert(u.app.clone());
        }
    }
    let mut out: Vec<String> = set.into_iter().collect();
    out.sort();
    out
}

fn to_descriptor(f: &Feature) -> ToolDescriptor {
    ToolDescriptor {
        id: f.id.clone(),
        title: f.title.clone(),
        summary: f.summary.clone(),
        capability: f.capability.to_string(),
        apps: f
            .uses
            .iter()
            .map(|u| AppRef {
                app: u.app.clone(),
                tool: u.tool.clone(),
                status: u.status,
            })
            .collect(),
    }
}
