//! A file the person might install, described from what its host said, before anything is
//! downloaded.

use std::path::PathBuf;

use craft_ai_features::{Licence, ModelFile, Source};

use super::format::Format;
use super::hosts::SourceKind;

/// Largest file believed, in bytes. A host that reports more is wrong or hostile.
pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024 * 1024;

/// Licences that are plainly permissive. Everything else, and anything unknown, has to be shown
/// to the person and accepted before the file is installed.
const PERMISSIVE: &[&str] = &[
    "apache-2.0",
    "mit",
    "bsd-2-clause",
    "bsd-3-clause",
    "isc",
    "unlicense",
    "cc0-1.0",
];

/// Whether a licence called `name` has to be accepted by the person.
pub fn licence_needs_accepting(name: &str) -> bool {
    !PERMISSIVE.contains(&name.trim().to_ascii_lowercase().as_str())
}

/// What a person must agree to, or supply, before the file is installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Needs {
    /// Accept the licence.
    pub licence: bool,
    /// Install although the host published no checksum to verify against.
    pub unverified: bool,
    /// Install a format that can run code when loaded.
    pub pickle: bool,
    /// Supply a token: the repository is gated.
    pub token: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub kind: SourceKind,
    /// The address the file is fetched from, pinned to a commit where the host has them. `None`
    /// for a file on this computer.
    pub url: Option<String>,
    /// A commit, tag or version that pins what was described.
    pub pin: String,
    /// The folder under ComfyUI's `models` this kind of file usually goes in. Empty when the host
    /// does not say; the person chooses.
    pub folder: String,
    pub filename: String,
    pub size_bytes: u64,
    /// The SHA-256 the host published. `None` when it published none.
    pub sha256: Option<String>,
    /// The format the name claims; the bytes are checked again at install.
    pub format: Format,
    pub licence: Licence,
    /// The host requires an account that has accepted its terms.
    pub gated: bool,
    pub local_path: Option<PathBuf>,
}

impl Candidate {
    pub fn needs(&self) -> Needs {
        Needs {
            licence: self.licence.accept_required,
            unverified: self.sha256.is_none() && self.kind != SourceKind::Local,
            // The name decides what is installed and checked, so it decides the consent too; a
            // `format` field that disagrees with the name can only add the requirement.
            pickle: self.format.runs_code_when_loaded()
                || Format::from_name(&self.filename).runs_code_when_loaded(),
            token: self.gated,
        }
    }

    pub fn download_url(&self) -> Option<String> {
        self.url.clone()
    }

    /// The downloader's view of this file, into `folder`. The checksum is empty when none was
    /// published; only `Downloader::fetch_unverified` accepts that.
    pub fn model_file(&self, folder: &str) -> Option<ModelFile> {
        Some(ModelFile {
            folder: folder.to_owned(),
            filename: self.filename.clone(),
            size_bytes: self.size_bytes,
            sha256: self.sha256.clone().unwrap_or_default(),
            source: Source::Url {
                url: self.url.clone()?,
            },
            licence: self.licence.clone(),
        })
    }
}
