//! Pinning a GitHub repository to one commit and describing it for the person.

use craft_ai_core::{AiError, SecretString};
use serde::Deserialize;
use serde_json::Value;

use super::{PackCandidate, PackSpec};
use crate::source::hosts::SourceKind;
use crate::source::resolve::Resolver;
use crate::source::resolve::api::{Api, is_hex, join, refused, tidy};
use crate::source::spec::revision_ok;

impl Resolver {
    /// Pins `spec` to a commit. Public repositories only: a private one is refused, because
    /// this app never holds a GitHub token. Nothing is downloaded.
    pub fn resolve_pack(
        &self,
        spec: &PackSpec,
        token: Option<&SecretString>,
    ) -> Result<PackCandidate, AiError> {
        let kind = SourceKind::NodePack;
        let settings = self.settings();
        let api = Api::new(settings);
        let (owner, repo) = (spec.owner.as_str(), spec.repo.as_str());
        let info = api.get(
            kind,
            &join(&settings.bases.github_api, &["repos", owner, repo])?,
            token,
        )?;
        if info.get("private").and_then(Value::as_bool) != Some(false) {
            return Err(refused("only public repositories can be installed"));
        }
        let reference = spec
            .reference
            .clone()
            .or_else(|| {
                info.get("default_branch")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .filter(|r| revision_ok(r))
            .ok_or_else(|| refused("the repository names no branch to install"))?;
        let commit = api.get(
            kind,
            &join(
                &settings.bases.github_api,
                &["repos", owner, repo, "commits", &reference],
            )?,
            token,
        )?;
        let sha = commit
            .get("sha")
            .and_then(Value::as_str)
            .filter(|s| is_hex(s, 40) && s.bytes().all(|b| !b.is_ascii_uppercase()))
            .ok_or_else(|| refused("the source did not name a commit"))?;
        // GitHub serves any commit of a repository's forks under the repository's own name, and
        // reads many spellings as a commit (an id of any length, `<name>-<n>-g<id>` and more), so
        // a pasted reference could be a fork's code shown as this repository's. Only a reference
        // that is one of the repository's own branches or tags, pointing at this very commit,
        // skips the check; anything else is accepted only when the default branch contains it.
        let own = match spec.reference.as_deref() {
            None => true,
            Some(r) => names_own_ref(
                &api,
                &settings.bases.github_api,
                token,
                [owner, repo],
                r,
                sha,
            )?,
        };
        if !own {
            let main = info
                .get("default_branch")
                .and_then(Value::as_str)
                .filter(|b| revision_ok(b))
                .ok_or_else(|| refused("the repository names no branch to install"))?;
            let comparison: Comparison = api.get_streamed(
                kind,
                &join(
                    &settings.bases.github_api,
                    &["repos", owner, repo, "compare", &format!("{main}...{sha}")],
                )?,
                token,
                MAX_COMPARE_BYTES,
            )?;
            if !matches!(comparison.status.as_str(), "behind" | "identical") {
                return Err(refused(
                    "this commit is not part of the repository's main line, so it may be someone else's copy; paste the address of a branch or tag instead",
                ));
            }
        }
        let subject = commit
            .pointer("/commit/message")
            .and_then(Value::as_str)
            .map(|m| m.lines().next().unwrap_or_default())
            .map(subject_line)
            .unwrap_or_else(|| "(no message)".to_owned());
        let committed_at = commit
            .pointer("/commit/committer/date")
            .and_then(Value::as_str)
            .map(|d| d.chars().filter(|c| !c.is_control()).take(40).collect())
            .unwrap_or_else(|| "unknown".to_owned());
        let licence = info
            .pointer("/license/spdx_id")
            .and_then(Value::as_str)
            .filter(|id| *id != "NOASSERTION")
            .map_or_else(|| "unknown".to_owned(), tidy);
        Ok(PackCandidate {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            reference,
            commit: sha.to_owned(),
            subject,
            committed_at,
            licence,
            archived: info.get("archived").and_then(Value::as_bool) == Some(true),
            folder: repo.to_owned(),
            archive_url: join(
                &settings.bases.github_web,
                &[owner, repo, "archive", &format!("{sha}.tar.gz")],
            )?,
        })
    }
}

/// Largest compare reply read. Only its status is kept, so memory stays small.
const MAX_COMPARE_BYTES: u64 = 32 * 1024 * 1024;

/// The one field of GitHub's compare reply that matters: how the pasted commit stands
/// against the default branch. `behind` or `identical` means the branch contains it.
#[derive(Deserialize)]
struct Comparison {
    status: String,
}

/// Most annotated tags followed before a tag is treated as not pointing at the commit.
const MAX_TAG_DEPTH: usize = 4;

/// Whether `reference` is exactly one of the repository's own branches or tags and points at
/// `sha`. GitHub's single-ref lookup matches the whole name, never a prefix, and answers 404
/// for a name that is no such ref. An annotated tag is followed to the commit it marks. Any
/// other failure is an error, so the caller never skips the check on a reply it could not read.
fn names_own_ref(
    api: &Api<'_>,
    base: &str,
    token: Option<&SecretString>,
    [owner, repo]: [&str; 2],
    reference: &str,
    sha: &str,
) -> Result<bool, AiError> {
    for space in ["heads", "tags"] {
        let address = join(
            base,
            &["repos", owner, repo, "git", "ref", space, reference],
        )?;
        let Some(found) = api.get_if_found(SourceKind::NodePack, &address, token)? else {
            continue;
        };
        let mut target = found.get("object").cloned().unwrap_or(Value::Null);
        for _ in 0..MAX_TAG_DEPTH {
            if target.get("type").and_then(Value::as_str) != Some("tag") {
                break;
            }
            let Some(tag) = target
                .get("sha")
                .and_then(Value::as_str)
                .filter(|s| is_hex(s, 40))
            else {
                break;
            };
            let address = join(base, &["repos", owner, repo, "git", "tags", tag])?;
            target = api
                .get(SourceKind::NodePack, &address, token)?
                .get("object")
                .cloned()
                .unwrap_or(Value::Null);
        }
        if target.get("type").and_then(Value::as_str) == Some("commit")
            && target.get("sha").and_then(Value::as_str) == Some(sha)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// One short clean line: no control characters, at most 120 characters.
fn subject_line(line: &str) -> String {
    let clean: String = line.chars().filter(|c| !c.is_control()).take(120).collect();
    let clean = clean.trim();
    if clean.is_empty() {
        "(no message)".to_owned()
    } else {
        clean.to_owned()
    }
}
