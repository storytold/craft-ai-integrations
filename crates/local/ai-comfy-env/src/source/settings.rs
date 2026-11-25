//! Where the registry talks to, and how strictly. The defaults are the real hosts; tests point
//! them at scripted loopback servers.

use std::time::Duration;

use super::hosts::SourceKind;
use crate::download::DownloadConfig;

/// The addresses of each host's listing service and, for GitHub, its web host.
#[derive(Clone, Debug)]
pub struct ApiBases {
    pub huggingface: String,
    pub github_api: String,
    pub github_web: String,
    pub civitai: String,
}

impl Default for ApiBases {
    fn default() -> Self {
        Self {
            huggingface: "https://huggingface.co".to_owned(),
            github_api: "https://api.github.com".to_owned(),
            github_web: "https://github.com".to_owned(),
            civitai: "https://civitai.com".to_owned(),
        }
    }
}

/// Replaces the built-in host rules of every kind. For tests, which have only loopback hosts.
#[derive(Clone, Debug)]
pub struct HostOverride {
    pub download: Vec<String>,
    pub token_host: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SourceSettings {
    pub bases: ApiBases,
    pub hosts: Option<HostOverride>,
    /// Permit loopback addresses. For tests.
    pub allow_loopback: bool,
    pub connect_timeout: Duration,
    pub retry_delay: Duration,
}

impl Default for SourceSettings {
    fn default() -> Self {
        Self {
            bases: ApiBases::default(),
            hosts: None,
            allow_loopback: false,
            connect_timeout: Duration::from_secs(15),
            retry_delay: Duration::from_secs(2),
        }
    }
}

impl SourceSettings {
    /// Hosts a download of this kind may use, redirects included.
    pub fn download_hosts(&self, kind: SourceKind) -> Vec<String> {
        match &self.hosts {
            Some(over) => over.download.clone(),
            None => kind
                .rules()
                .download
                .iter()
                .map(|h| (*h).to_owned())
                .collect(),
        }
    }

    /// The one host that may receive a token for this kind.
    pub fn token_host(&self, kind: SourceKind) -> Option<String> {
        match &self.hosts {
            Some(over) => over.token_host.clone(),
            None => kind.rules().token_host.map(str::to_owned),
        }
    }

    /// A downloader for this kind: limited to its hosts, token for its token host only.
    pub(crate) fn download_config(&self, kind: SourceKind) -> DownloadConfig {
        DownloadConfig {
            connect_timeout: self.connect_timeout,
            retry_delay: self.retry_delay,
            // A kind with no token host gets a name no address can have.
            auth_host: self
                .token_host(kind)
                .unwrap_or_else(|| "no-token.invalid".to_owned()),
            allowed_hosts: Some(self.download_hosts(kind)),
            allow_loopback: self.allow_loopback,
            ..DownloadConfig::default()
        }
    }
}
