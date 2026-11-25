//! What a pasted address means: a Hugging Face file, a GitHub release asset, a Civitai model
//! version, or a file on this computer.
//!
//! Only exact, https addresses of those three hosts are understood; anything else is refused with
//! a reason rather than guessed at. Path pieces are percent-decoded and then checked, so
//! `%2e%2e` and `%2F` cannot smuggle a path out of the folder.

use std::path::{Component, Path, PathBuf};

use percent_encoding::percent_decode_str;
use url::Url;

use super::hosts::SourceKind;

const MAX_INPUT_BYTES: usize = 2048;
const MAX_SEGMENT_BYTES: usize = 255;

/// A source, as far as the address says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceSpec {
    HuggingFace {
        /// `owner/name`.
        repo: String,
        /// Path of the file inside the repository.
        file: String,
        /// A branch, tag or commit; the installer pins it to a commit.
        revision: String,
    },
    GitHubRelease {
        owner: String,
        repo: String,
        tag: String,
        asset: String,
    },
    Civitai {
        version_id: u64,
    },
    Local {
        path: PathBuf,
    },
}

/// Why an address was not understood.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecError {
    Empty,
    TooLong,
    /// Not Hugging Face, a GitHub release, Civitai, or an absolute path on this computer.
    NotSupported,
    /// Plain http, or a user name in the address.
    Insecure,
    /// A Civitai model address without a version.
    NeedsVersion,
    /// A repository or release address that names no file.
    NeedsFile,
    /// A part of the path is not a plain name.
    BadName,
}

impl SourceSpec {
    /// Checks every piece again. [`parse`] already does; this is for a spec built by hand.
    pub fn validate(&self) -> Result<(), SpecError> {
        let ok = match self {
            Self::HuggingFace {
                repo,
                file,
                revision,
            } => {
                let mut halves = repo.split('/');
                let repo_ok = matches!(
                    (halves.next(), halves.next(), halves.next()),
                    (Some(a), Some(b), None) if plain(a) && plain(b)
                );
                repo_ok && revision_ok(revision) && file.split('/').all(plain)
            }
            Self::GitHubRelease {
                owner,
                repo,
                tag,
                asset,
            } => [owner, repo, tag, asset].into_iter().all(|s| plain(s)),
            Self::Civitai { .. } => true,
            Self::Local { path } => {
                path.is_absolute() && !path.components().any(|c| c == Component::ParentDir)
            }
        };
        if ok { Ok(()) } else { Err(SpecError::BadName) }
    }

    /// The name of the file the address asks for, when the address says it.
    pub fn file_name(&self) -> Option<String> {
        match self {
            Self::HuggingFace { file, .. } => file.rsplit('/').next().map(str::to_owned),
            Self::GitHubRelease { asset, .. } => Some(asset.clone()),
            Self::Civitai { .. } => None,
            Self::Local { path } => path.file_name()?.to_str().map(str::to_owned),
        }
    }

    pub fn kind(&self) -> SourceKind {
        match self {
            Self::HuggingFace { .. } => SourceKind::HuggingFace,
            Self::GitHubRelease { .. } => SourceKind::GitHubRelease,
            Self::Civitai { .. } => SourceKind::Civitai,
            Self::Local { .. } => SourceKind::Local,
        }
    }
}

/// Understands one pasted address.
pub fn parse(input: &str) -> Result<SourceSpec, SpecError> {
    let text = input.trim();
    if text.is_empty() {
        return Err(SpecError::Empty);
    }
    if text.len() > MAX_INPUT_BYTES {
        return Err(SpecError::TooLong);
    }
    // Before URL parsing: `C:\models\m.safetensors` would otherwise read as scheme `c`.
    let path = Path::new(text);
    if path.is_absolute() {
        if path.components().any(|c| c == Component::ParentDir) {
            return Err(SpecError::BadName);
        }
        return Ok(SourceSpec::Local {
            path: path.to_owned(),
        });
    }
    let url = Url::parse(text).map_err(|_| SpecError::NotSupported)?;
    match url.scheme() {
        "https" => {}
        "http" => return Err(SpecError::Insecure),
        _ => return Err(SpecError::NotSupported),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(SpecError::Insecure);
    }
    if url.port().is_some() {
        return Err(SpecError::NotSupported);
    }
    let segments = segments(&url)?;
    let parts: Vec<&str> = segments.iter().map(String::as_str).collect();
    match url.host_str() {
        Some("huggingface.co") => hugging_face(&parts),
        Some("github.com") => github(&parts),
        Some("civitai.com") => civitai(&parts, &url),
        _ => Err(SpecError::NotSupported),
    }
}

/// The decoded path pieces, each a plain name.
pub(crate) fn segments(url: &Url) -> Result<Vec<String>, SpecError> {
    let mut raw: Vec<&str> = url
        .path_segments()
        .ok_or(SpecError::NotSupported)?
        .collect();
    // A trailing slash leaves one empty piece at the end; an empty piece anywhere else is a mistake.
    if raw.len() > 1 && raw.last() == Some(&"") {
        raw.pop();
    }
    raw.into_iter()
        .map(|piece| {
            let decoded = percent_decode_str(piece)
                .decode_utf8()
                .map_err(|_| SpecError::BadName)?;
            if plain(&decoded) {
                Ok(decoded.into_owned())
            } else {
                Err(SpecError::BadName)
            }
        })
        .collect()
}

/// Whether `name` is one plain file or folder name.
pub(crate) fn plain_name(name: &str) -> bool {
    plain(name)
}

fn plain(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= MAX_SEGMENT_BYTES
        && !segment.starts_with('.')
        && !segment
            .chars()
            .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
}

pub(crate) fn revision_ok(revision: &str) -> bool {
    !revision.is_empty()
        && revision.len() <= 100
        && revision
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn hugging_face(parts: &[&str]) -> Result<SourceSpec, SpecError> {
    match parts {
        [_, _] | [_, _, "tree", ..] | [_, _, "blob" | "resolve", _] => Err(SpecError::NeedsFile),
        [owner, name, "blob" | "resolve", revision, file @ ..] if !file.is_empty() => {
            if !revision_ok(revision) {
                return Err(SpecError::BadName);
            }
            Ok(SourceSpec::HuggingFace {
                repo: format!("{owner}/{name}"),
                file: file.join("/"),
                revision: (*revision).to_owned(),
            })
        }
        _ => Err(SpecError::NotSupported),
    }
}

fn github(parts: &[&str]) -> Result<SourceSpec, SpecError> {
    match parts {
        [owner, repo, "releases", "download", tag, asset] => Ok(SourceSpec::GitHubRelease {
            owner: (*owner).to_owned(),
            repo: (*repo).to_owned(),
            tag: (*tag).to_owned(),
            asset: (*asset).to_owned(),
        }),
        [_, _, "releases", "download", _] | [_, _, "releases", "download"] => {
            Err(SpecError::NeedsFile)
        }
        _ => Err(SpecError::NotSupported),
    }
}

fn civitai(parts: &[&str], url: &Url) -> Result<SourceSpec, SpecError> {
    let number = |text: &str| text.parse::<u64>().map_err(|_| SpecError::BadName);
    match parts {
        ["api", "download", "models", version] => Ok(SourceSpec::Civitai {
            version_id: number(version)?,
        }),
        ["models", id, ..] => {
            number(id)?;
            let version = url
                .query_pairs()
                .find(|(key, _)| key == "modelVersionId")
                .ok_or(SpecError::NeedsVersion)?;
            Ok(SourceSpec::Civitai {
                version_id: number(&version.1)?,
            })
        }
        _ => Err(SpecError::NotSupported),
    }
}
