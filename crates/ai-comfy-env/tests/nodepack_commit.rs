#![allow(dead_code)]
//! A pasted node pack address that names something other than one of the repository's own
//! branches or tags. GitHub serves a fork's commits under the original repository's name, and
//! reads many spellings as a commit, so such an address could show a fork's code as the trusted
//! repository's. These tests pin that only an exact branch or tag pointing at the resolved commit
//! skips the check, and that anything else is accepted only when GitHub's compare says the
//! repository's default branch contains it. Scripted localhost servers only: no real repository
//! is read.
mod nodepack_rig;

use craft_ai_comfy_env::source::{PackCandidate, Resolver, parse_pack};
use craft_ai_core::AiError;
use nodepack_rig::support::{Reply, Server};
use nodepack_rig::*;
use serde_json::{Value, json};

/// Another commit, for a branch that exists but points elsewhere.
const OTHER: &str = "fedcba9876543210fedcba9876543210fedcba98";
/// An annotated tag object.
const TAG_OBJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// The compare path the resolver must ask for a reference that is not the repository's own.
fn compare_path() -> String {
    format!("/repos/owner/Pack/compare/main...{COMMIT}")
}

/// One of the repository's own refs: `space` is `heads` or `tags`, `object` what it points at.
struct Ref {
    space: &'static str,
    name: &'static str,
    object: Value,
}

fn branch(name: &'static str, sha: &str) -> Ref {
    Ref {
        space: "heads",
        name,
        object: json!({"type": "commit", "sha": sha}),
    }
}

fn tag(name: &'static str, object: Value) -> Ref {
    Ref {
        space: "tags",
        name,
        object,
    }
}

/// A stand-in for GitHub whose compare reply is `compare` (`None` answers 404 there) and whose
/// own branches and tags are `refs`. Every commit lookup resolves to [`COMMIT`].
fn github_with(compare: Option<String>, refs: Vec<Ref>) -> Server {
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
        } else if r.path == format!("/repos/owner/Pack/git/tags/{TAG_OBJECT}") {
            Reply::Json(
                200,
                json!({"object": {"type": "commit", "sha": COMMIT}}).to_string(),
            )
        } else if let Some(found) = refs
            .iter()
            .find(|f| r.path == format!("/repos/owner/Pack/git/ref/{}/{}", f.space, f.name))
        {
            Reply::Json(
                200,
                json!({"ref": format!("refs/{}/{}", found.space, found.name),
                       "object": found.object})
                .to_string(),
            )
        } else {
            Reply::Json(404, "{}".into())
        }
    })
}

/// A stand-in with no branches or tags besides the default one.
fn github(compare: Option<String>) -> Server {
    github_with(compare, Vec::new())
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

fn at(reference: &str) -> String {
    format!("https://github.com/owner/Pack/tree/{reference}")
}

#[test]
fn a_commit_on_the_default_branch_is_accepted() {
    for s in ["behind", "identical"] {
        let server = github(status(s));
        let c = resolve(&server, &at(COMMIT)).unwrap();
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
        let e = resolve(&server, &at(COMMIT)).unwrap_err();
        assert!(e.message.contains("main line"), "{s}: {}", e.message);
    }
}

#[test]
fn a_commit_is_refused_when_the_comparison_cannot_be_read() {
    for compare in [None, Some("{}".to_owned()), Some("not json".to_owned())] {
        let server = github(compare.clone());
        assert!(resolve(&server, &at(COMMIT)).is_err(), "{compare:?}");
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
    let c = resolve(&server, &at(COMMIT)).unwrap();
    assert_eq!(c.commit, COMMIT);
}

#[test]
fn a_comparison_longer_than_the_limit_is_refused_even_when_its_start_parses() {
    // A complete answer followed by padding: what was read parses, but more was left unread.
    const LIMIT: usize = 32 * 1024 * 1024;
    let head = r#"{"status":"behind"}"#;
    let exact = format!("{head}{}", " ".repeat(LIMIT - head.len()));
    let server = github(Some(exact));
    assert_eq!(resolve(&server, &at(COMMIT)).unwrap().commit, COMMIT);
    let over = format!("{head}{}", " ".repeat(LIMIT + 1 - head.len()));
    let server = github(Some(over));
    let e = resolve(&server, &at(COMMIT)).unwrap_err();
    assert!(e.message.contains("too large"), "{}", e.message);
}

#[test]
fn a_short_commit_id_of_any_length_is_checked_too() {
    // GitHub accepts any unambiguous prefix, so a six- or four-character id must not slip past
    // the check that a seven-character one gets.
    for short in ["0123456", "012345", "0123", "0123456789AB"] {
        let server = github(status("ahead"));
        let e = resolve(&server, &at(short)).unwrap_err();
        assert!(e.message.contains("main line"), "{short}: {}", e.message);
        let server = github(status("behind"));
        let c = resolve(&server, &at(short)).unwrap();
        assert_eq!(c.commit, COMMIT, "{short}");
        assert!(paths(&server).contains(&compare_path()), "{short}");
    }
}

#[test]
fn any_other_spelling_github_reads_as_a_commit_is_checked() {
    // GitHub also reads `git describe` output (`<name>-<n>-g<id>`) and names like `HEAD` as a
    // commit. None of these is one of the repository's own branches or tags, so each must reach
    // the compare and be refused when the default branch does not contain the commit.
    for reference in [
        "a-1-g0123456",
        "0-0-g0123456789ab",
        "v1.0.0-3-g01234567",
        "main-0-g0123",
        "HEAD",
        "0123456789abcdef0123456789abcdef01234567",
    ] {
        let server = github(status("ahead"));
        let e = resolve(&server, &at(reference)).unwrap_err();
        assert!(
            e.message.contains("main line"),
            "{reference}: {}",
            e.message
        );
        assert!(paths(&server).contains(&compare_path()), "{reference}");
    }
}

#[test]
fn a_branch_that_points_at_another_commit_is_checked() {
    // The branch exists, but the commit GitHub resolved is not the one it points at, so the
    // name was not read as that branch.
    let server = github_with(
        status("ahead"),
        vec![
            branch("dev", OTHER),
            tag("dev", json!({"type": "commit", "sha": OTHER})),
        ],
    );
    let e = resolve(&server, &at("dev")).unwrap_err();
    assert!(e.message.contains("main line"), "{}", e.message);
}

#[test]
fn a_failed_branch_lookup_is_never_taken_as_the_repositorys_own() {
    // Only a 404 means "no such branch"; a server error stops the install.
    let server = Server::start(|r| {
        if r.path.contains("/git/ref/") {
            Reply::Json(500, "{}".into())
        } else if r.path == "/repos/owner/Pack" {
            Reply::Json(
                200,
                json!({"default_branch": "main", "private": false}).to_string(),
            )
        } else if r.path.starts_with("/repos/owner/Pack/commits/") {
            Reply::Json(200, json!({"sha": COMMIT}).to_string())
        } else {
            Reply::Json(200, json!({"status": "behind"}).to_string())
        }
    });
    assert!(resolve(&server, &at("dev")).is_err());
}

#[test]
fn branch_and_tag_names_and_the_default_branch_need_no_comparison() {
    // Each of these is one of the repository's own branches or tags pointing at the resolved
    // commit, even the ones made of hex digits, so no comparison is asked for.
    let refs = || {
        vec![
            branch("dev-1.2", COMMIT),
            branch("abc123", COMMIT),
            branch("fedcba9", COMMIT),
            tag("2024", json!({"type": "commit", "sha": COMMIT})),
            tag("v1.0.0", json!({"type": "tag", "sha": TAG_OBJECT})),
        ]
    };
    for address in [
        "https://github.com/owner/Pack".to_owned(),
        at("dev-1.2"),
        at("v1.0.0"),
        at("abc123"),
        at("2024"),
        at("fedcba9"),
    ] {
        let server = github_with(None, refs());
        resolve(&server, &address).unwrap();
        assert!(
            !paths(&server).iter().any(|p| p.contains("/compare/")),
            "{address}"
        );
    }
}
