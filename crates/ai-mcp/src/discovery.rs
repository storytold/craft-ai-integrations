//! Dynamic discovery of Craft apps across the storytold org's public repositories.
//!
//! The shared catalogue is the source of truth for *which* tools exist. This module fetches the
//! org's public repos so newly landed apps can be noticed; it does not itself surface them to MCP
//! hosts — `tools/list` reflects only the shared catalogue until a new app is added there.

use std::io::Read;
use std::time::Duration;

use serde::Deserialize;

/// Largest response body read, in bytes. The org repos list is small; this bounds a hostile reply.
const MAX_BODY: usize = 4 * 1024 * 1024;

/// Maximum number of pages fetched when following GitHub's `Link` header (`rel="next"`), so an
/// org with more than 100 repositories returns the complete list rather than just the first page.
/// Each page carries at most 100 repos (GitHub's per_page cap), so this bounds memory and requests.
const MAX_PAGES: usize = 5;

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

/// Fetches repositories from an explicit URL, following GitHub's `Link` header (`rel="next"`)
/// up to [`MAX_PAGES`] pages so an org with more than 100 repositories returns the complete list.
/// The public [`fetch_org_repos`] is a convenience wrapper for the GitHub org endpoint; this form
/// lets tests point at a scripted localhost server.
pub fn fetch_from(url: &str) -> Result<Vec<OrgRepo>, String> {
    let config = ureq::Agent::config_builder()
        .tls_config(craft_ai_http::tls_config())
        .proxy(craft_ai_http::proxy_for(url))
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent("craft-ai-mcp/0.1")
        .build();
    let agent = ureq::Agent::new_with_config(config);

    let mut repos: Vec<OrgRepo> = Vec::new();
    let mut current_url = url.to_string();
    for _ in 0..MAX_PAGES {
        let resp = agent.get(&current_url).call().map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("GitHub returned HTTP {}", resp.status()));
        }
        // Extract the next-page link before consuming the body.
        let next = next_page_url(resp.headers().get("link").and_then(|v| v.to_str().ok()));
        let bytes = read_capped(resp.into_body().into_reader(), MAX_BODY)?;
        let page: Vec<OrgRepo> = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        repos.extend(page);
        current_url = match next {
            Some(u) => u,
            None => return Ok(repos),
        };
    }
    // Another page remains: a partial list must not pass for the whole org.
    Err(format!(
        "the organisation lists more than {MAX_PAGES} pages of repositories; the list is incomplete"
    ))
}

/// The `rel="next"` URL from a GitHub `Link` header, if present. A header may list several
/// comma-separated entries; only the one with `rel="next"` is followed.
fn next_page_url(link_header: Option<&str>) -> Option<String> {
    link_header?
        .split(',')
        .map(|part| part.trim())
        .find_map(|part| {
            if !part.contains("rel=\"next\"") && !part.contains("rel='next'") {
                return None;
            }
            let lt = part.find('<')?;
            let start = lt + 1;
            let gt = part[start..].find('>')?;
            Some(part[start..start + gt].to_string())
        })
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
