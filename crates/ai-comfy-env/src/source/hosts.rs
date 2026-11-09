//! The hosts each kind of model source may use.
//!
//! A source kind owns a short list of hosts: the one that serves the listing and the files, and
//! the storage hosts those redirect to. A download that is sent anywhere else is refused, and a
//! token is only ever sent to the single host named by [`HostRules::token_host`]. Matching is on
//! the host exactly, or on a subdomain when the pattern starts with a dot, never on a suffix of
//! the text (`evilhuggingface.co` is not `huggingface.co`).

use serde::{Deserialize, Serialize};

/// Where a model file comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    HuggingFace,
    GitHubRelease,
    Civitai,
    /// A ComfyUI node pack, as an archive of one commit of a GitHub repository.
    NodePack,
    /// A file on this computer.
    Local,
}

/// The hosts a kind of source uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostRules {
    /// Hosts a download may be fetched from, redirects included. `.example.com` means any
    /// subdomain of it.
    pub download: &'static [&'static str],
    /// The only host that may receive a token. `None` when this kind takes no token.
    pub token_host: Option<&'static str>,
}

impl SourceKind {
    /// The kind's hosts. The storage hosts for GitHub and Hugging Face are the documented
    /// content-delivery domains; Civitai's is the object store its download links redirect to.
    /// None of these has been checked against the live service from this code.
    pub fn rules(self) -> HostRules {
        match self {
            Self::HuggingFace => HostRules {
                download: &["huggingface.co", ".huggingface.co", ".hf.co"],
                token_host: Some("huggingface.co"),
            },
            // The resolver reads release metadata and licences from the API before downloading
            // public assets from the web/content hosts. No token is ever sent to GitHub.
            Self::GitHubRelease => HostRules {
                download: &["api.github.com", "github.com", ".githubusercontent.com"],
                token_host: None,
            },
            Self::Civitai => HostRules {
                download: &["civitai.com", ".civitai.com", ".r2.cloudflarestorage.com"],
                token_host: Some("civitai.com"),
            },
            // The archive address redirects to GitHub's code host, a subdomain of github.com;
            // the listing service is another. No token is ever sent.
            Self::NodePack => HostRules {
                download: &["github.com", ".github.com"],
                token_host: None,
            },
            Self::Local => HostRules {
                download: &[],
                token_host: None,
            },
        }
    }

    /// A short name for a screen.
    pub fn label(self) -> &'static str {
        match self {
            Self::HuggingFace => "Hugging Face",
            Self::GitHubRelease => "GitHub release",
            Self::Civitai => "Civitai",
            Self::NodePack => "Node pack from GitHub",
            Self::Local => "A file on this computer",
        }
    }
}

/// Whether `host` is the host `pattern` names, or (for `.pattern`) a subdomain of it. Case is
/// ignored. A trailing dot, an empty host or an empty pattern never matches.
pub fn host_matches(pattern: &str, host: &str) -> bool {
    if pattern.is_empty() || host.is_empty() {
        return false;
    }
    let host = host.to_ascii_lowercase();
    let pattern = pattern.to_ascii_lowercase();
    match pattern.strip_prefix('.') {
        Some(rest) if !rest.is_empty() => {
            host.len() > rest.len() + 1 && host.ends_with(&format!(".{rest}"))
        }
        Some(_) => false,
        None => host == pattern,
    }
}
