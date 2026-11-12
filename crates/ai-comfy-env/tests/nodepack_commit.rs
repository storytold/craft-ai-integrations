#![allow(dead_code)]
//! A pasted node pack address that names a commit id. GitHub serves a fork's commits under the
//! original repository's name, so a commit id alone could show a fork's code as the trusted
//! repository's. These tests pin that a commit id is accepted only when GitHub's compare says
//! the repository's default branch contains it, and that branch and tag names need no extra
//! check. Scripted localhost servers only: no real repository is read.
mod nodepack_rig;

use craft_ai_comfy_env::source::{PackCandidate, Resolver, parse_pack};
use craft_ai_core::AiError;
use nodepack_rig::support::{Reply, Server};
use nodepack_rig::*;
use serde_json::json;

/// The compare path the resolver must ask for a pasted commit.
fn compare_path() -> String {
    format!("/repos/owner/Pack/compare/main...{COMMIT}")
}

/// A stand-in for GitHub whose compare reply is `compare`; `None` answers 404 there.
fn github(compare: Option<String>) -> Server {
    Server::start(move |r| {
        if r.path == "/repos/owner/Pack" {
            Reply::Json(
                200,
                json!({"default_branch": "main", "private": false, "archived": false,
                       "license": {"spdx_id": "MIT"}})
                .to_string(),
            )
        } else if r.path.starts_with("/repos/owner/Pack/commits/") {
            Reply::Json(
                200,
                json!({"sha": COMMIT, "commit": {"message": "Add the nodes",
                       "committer": {"date": "2026-10-01T12:00:00Z"}}})
                .to_string(),
            )
        } else if r.path == compare_path() {
            match &compare {
                Some(body) => Reply::Json(200, body.clone()),
                None => Reply::Json(404, "{}".into()),
            }
        } else {
            Reply::Json(404, "{}".into())
        }
    })
}

/// A compare reply with this status.
fn status(s: &str) -> Option<String> {
    Some(json!({"status": s, "ahead_by": 0, "behind_by": 3, "files": []}).to_string())
}

/// Resolves `address` against `server`.
fn resolve(server: &Server, address: &str) -> Result<PackCandidate, AiError> {
    Resolver::new(settings(&origin(server), &["127.0.0.1"]))
        .resolve_pack(&parse_pack(address).unwrap(), None)
}

/// The paths the stand-in was asked for.
fn paths(server: &Server) -> Vec<String> {
    server.recorded().iter().map(|r| r.path.clone()).collect()
}

#[test]
fn a_commit_on_the_default_branch_is_accepted() {
    for s in ["behind", "identical"] {
        let server = github(status(s));
        let c = resolve(
            &server,
            &format!("https://github.com/owner/Pack/tree/{COMMIT}"),
        )
        .unwrap();
        assert_eq!(c.commit, COMMIT);
        assert_eq!(c.reference, COMMIT);
        assert!(paths(&server).contains(&compare_path()), "{s}");
    }
}

#[test]
fn a_commit_the_default_branch_does_not_contain_is_refused() {
    // `ahead` or `diverged` means the commit is not on the branch: a fork's own commit, or
    // one only on another branch. Anything else GitHub might say is refused too.
    for s in ["ahead", "diverged", "", "Behind"] {
        let server = github(status(s));
        let e = resolve(
            &server,
            &format!("https://github.com/owner/Pack/tree/{COMMIT}"),
        )
        .unwrap_err();
        assert!(e.message.contains("main line"), "{s}: {}", e.message);
    }
}

#[test]
fn a_commit_is_refused_when_the_comparison_cannot_be_read() {
    for compare in [None, Some("{}".to_owned()), Some("not json".to_owned())] {
        let server = github(compare.clone());
        assert!(
            resolve(
                &server,
                &format!("https://github.com/owner/Pack/tree/{COMMIT}"),
            )
            .is_err(),
            "{compare:?}"
        );
    }
}

#[test]
fn a_long_comparison_is_read_without_keeping_it() {
    // An old commit compared with a busy branch lists many changed files: far more than the
    // 2 MiB a listing reply may be. Only the status is kept.
    let files: Vec<_> = (0..3000)
        .map(|i| json!({"filename": format!("nodes/{i}.py"), "patch": "x".repeat(1000)}))
        .collect();
    let body = json!({"status": "behind", "files": files}).to_string();
    assert!(body.len() > 2 * 1024 * 1024);
    let server = github(Some(body));
    let c = resolve(
        &server,
        &format!("https://github.com/owner/Pack/tree/{COMMIT}"),
    )
    .unwrap();
    assert_eq!(c.commit, COMMIT);
}

#[test]
fn a_short_commit_id_of_any_length_is_checked_too() {
    // GitHub accepts any unambiguous prefix, so a six- or four-character id must not slip past
    // the check that a seven-character one gets.
    for short in ["0123456", "012345", "0123", "0123456789AB"] {
        let server = github(status("ahead"));
        let e = resolve(
            &server,
            &format!("https://github.com/owner/Pack/tree/{short}"),
        )
        .unwrap_err();
        assert!(e.message.contains("main line"), "{short}: {}", e.message);
        let server = github(status("behind"));
        let c = resolve(
            &server,
            &format!("https://github.com/owner/Pack/tree/{short}"),
        )
        .unwrap();
        assert_eq!(c.commit, COMMIT, "{short}");
        assert!(paths(&server).contains(&compare_path()), "{short}");
    }
}

#[test]
fn branch_and_tag_names_and_the_default_branch_need_no_comparison() {
    // Each of these resolves to a commit that does not begin with the name, so GitHub read it
    // as one of the repository's own branches or tags, even the ones made of hex digits.
    for address in [
        "https://github.com/owner/Pack",
        "https://github.com/owner/Pack/tree/dev-1.2",
        "https://github.com/owner/Pack/tree/v1.0.0",
        "https://github.com/owner/Pack/tree/abc123",
        "https://github.com/owner/Pack/tree/2024",
        "https://github.com/owner/Pack/tree/fedcba9",
    ] {
        let server = github(None);
        resolve(&server, address).unwrap();
        assert!(
            !paths(&server).iter().any(|p| p.contains("/compare/")),
            "{address}"
        );
    }
}
