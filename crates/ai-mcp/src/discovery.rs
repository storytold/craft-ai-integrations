//! Dynamic discovery of Craft apps across the storytold org's public repositories.
//!
//! The shared catalogue is the source of truth for *which* tools exist. This module watches the
//! org's public repos to surface apps that have landed but are not yet listed in the catalogue,
//! so a new app (or tool it offers) shows up without editing this crate.

use std::io::Read;
use std::time::Duration;

use serde::Deserialize;

/// Largest response body read, in bytes. The org repos list is small; this bounds a hostile reply.
const MAX_BODY: usize = 4 * 1024 * 1024;

/// A repository as returned by the GitHub API.
#[derive(Debug, Deserialize)]
pub struct OrgRepo {
    pub name: String,
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_archived")]
    pub archived: bool,
}

fn default_archived() -> bool {
    false
}

/// Fetches the org's public repositories from GitHub.
pub fn fetch_org_repos(org: &str) -> Result<Vec<OrgRepo>, String> {
    let url = format!("https://api.github.com/orgs/{org}/repos?per_page=100&sort=updated");
    fetch_from(&url)
}

/// Fetches repositories from an explicit URL. The public [`fetch_org_repos`] is a convenience
/// wrapper for the GitHub org endpoint; this form lets tests point at a scripted localhost server.
pub fn fetch_from(url: &str) -> Result<Vec<OrgRepo>, String> {
    let config = ureq::Agent::config_builder()
        .tls_config(craft_ai_http::tls_config())
        .proxy(craft_ai_http::proxy_for(&url))
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent("craft-ai-mcp/0.1")
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let resp = agent.get(url).call().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("GitHub returned HTTP {}", resp.status()));
    }
    let bytes = read_capped(resp.into_body().into_reader(), MAX_BODY)?;
    let repos: Vec<OrgRepo> = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    Ok(repos)
}

/// Reads at most `max` bytes from a response body; more than that is an error rather than a
/// truncated reply (mirrors the bounded reads elsewhere in the workspace).
fn read_capped(reader: impl Read, max: usize) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    reader
        .take(limit)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    if buf.len() > max {
        return Err(format!("reply exceeds {} bytes", max));
    }
    Ok(buf)
}

/// Repos that look like Craft apps. Names matching `*craft` (photocraft) or `craft-*`
/// (craft-anything) are treated as Craft apps; archived repos are excluded. A description
/// mentioning "craft" is a bonus signal but never required, so an app whose name carries the
/// brand still surfaces even when its blurb doesn't repeat it.
pub fn craft_apps(repos: &[OrgRepo]) -> Vec<String> {
    repos
        .iter()
        .filter(|r| !r.archived && is_craft_app(r))
        .map(|r| r.name.clone())
        .collect()
}

fn is_craft_app(r: &OrgRepo) -> bool {
    r.name.to_lowercase().contains("craft")
}

/// Apps in the org that are not yet known to the shared catalogue (prompt to add them).
pub fn unknown_apps(catalogue_apps: &[String], org_apps: &[String]) -> Vec<String> {
    org_apps
        .iter()
        .filter(|a| !catalogue_apps.iter().any(|c| c.eq_ignore_ascii_case(a)))
        .cloned()
        .collect()
}
