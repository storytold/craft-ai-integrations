//! A media link that is clearly past its expiry is refused before any request, and a copy that
//! fails leaves the result's link with its expiry attached and a notice that says when it lapses.
//! Scripted localhost servers only.
#![allow(dead_code)]
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_core::{
    AiError, ErrorKind, Event, JobId, JobSnapshot, JobState, ModelId, ModelInfo, Output, Payload,
    PayloadHandle, Provider, Request,
};
use craft_ai_media::expiry::{CLOCK_GRACE_SECS, now_unix};
use craft_ai_media::{MAX_SAVES_PER_POLL, MediaStore, MediaStoreConfig, PersistingProvider};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::{Server, http_bytes};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR-fake-body";

/// A store config in `dir` that may reach localhost and does not wait between tries.
fn config(dir: &Path) -> MediaStoreConfig {
    let mut cfg = MediaStoreConfig::new(dir);
    cfg.allow_loopback = true;
    cfg.retry.first_delay = Duration::ZERO;
    cfg
}

/// The server's address without the `/v1` API suffix.
fn origin(server: &Server) -> String {
    server.url.trim_end_matches("/v1").to_owned()
}

/// A CloudFront-style signed link to the server that stops working at `expires`.
fn signed(server: &Server, expires: u64) -> PayloadHandle {
    let uri = format!(
        "{}/media/out.png?Expires={expires}&Signature=abc",
        origin(server)
    );
    let Payload::Handle(handle) = Payload::handle(&uri, "image/png", None).unwrap() else {
        unreachable!()
    };
    handle
}

/// Saves `handle` through a fresh store in `dir`. `server` is only kept alive by the caller.
fn save(server: &Server, dir: &Path, handle: &PayloadHandle) -> Result<PayloadHandle, AiError> {
    let _ = server;
    MediaStore::new(config(dir)).unwrap().save(handle)
}

/// A signed link well past its expiry is refused at once: no request, no retry.
#[test]
fn a_clearly_expired_signed_link_is_not_fetched() {
    let server = Server::start(|_| http_bytes(200, &[], PNG));
    let dir = tempfile::tempdir().unwrap();
    let past = now_unix() - CLOCK_GRACE_SECS - 60;
    let error = save(&server, dir.path(), &signed(&server, past)).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Provider);
    assert!(error.message.contains("expired at"), "{error:?}");
    assert!(error.message.ends_with("UTC"), "{error:?}");
    assert_eq!(server.count(), 0);
}

/// The handle's own expiry counts as well as the signed one.
#[test]
fn a_handle_that_says_it_expired_is_not_fetched() {
    let server = Server::start(|_| http_bytes(200, &[], PNG));
    let dir = tempfile::tempdir().unwrap();
    let uri = format!("{}/media/out.png", origin(&server));
    let Payload::Handle(mut handle) = Payload::handle(&uri, "image/png", None).unwrap() else {
        unreachable!()
    };
    handle.expires_at = Some(now_unix() - CLOCK_GRACE_SECS - 60);
    let error = save(&server, dir.path(), &handle).unwrap_err();
    assert!(error.message.contains("expired at"), "{error:?}");
    assert_eq!(server.count(), 0);
}

/// A link only just past its expiry is still tried, in case this computer's clock runs fast.
#[test]
fn a_link_inside_the_clock_grace_is_still_tried() {
    let server = Server::start(|_| http_bytes(200, &[], PNG));
    let dir = tempfile::tempdir().unwrap();
    let just_past = now_unix() - 60;
    save(&server, dir.path(), &signed(&server, just_past)).unwrap();
    assert_eq!(server.count(), 1);
}

/// Answers every poll with the scripted outputs, once.
struct Fake {
    outputs: Mutex<Vec<Payload>>,
}

impl Provider for Fake {
    /// No models.
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        Ok(Vec::new())
    }
    /// Always job `job-1`.
    fn submit(&self, _: &ModelId, _: &Request) -> Result<JobId, AiError> {
        JobId::new("job-1").map_err(|e| AiError::invalid(e.to_string()))
    }
    /// A succeeded snapshot holding the scripted outputs; later polls have none.
    fn poll(&self, _: &JobId) -> Result<JobSnapshot, AiError> {
        let mut snap = JobSnapshot::new(JobState::Succeeded);
        for payload in std::mem::take(&mut *self.outputs.lock().unwrap()) {
            snap.events.push(Event::Output(Output {
                payload,
                label: None,
            }));
        }
        Ok(snap)
    }
    /// Does nothing.
    fn cancel(&self, _: &JobId) -> Result<(), AiError> {
        Ok(())
    }
}

/// Polls a persisting provider once, with one try per copy, over a job whose only output is
/// `link`.
fn run(dir: &Path, link: PayloadHandle) -> JobSnapshot {
    run_all(dir, vec![link])
}

/// Polls a persisting provider once, with one try per copy, over a job whose outputs are
/// `links`.
fn run_all(dir: &Path, links: Vec<PayloadHandle>) -> JobSnapshot {
    let mut cfg = config(dir);
    cfg.retry.max_attempts = 1;
    let store = Arc::new(MediaStore::new(cfg).unwrap());
    let fake = Arc::new(Fake {
        outputs: Mutex::new(links.into_iter().map(Payload::Handle).collect()),
    });
    PersistingProvider::new(fake, store)
        .poll(&JobId::new("job-1").unwrap())
        .unwrap()
}

/// The first output's handle; panics if it is not a handle.
fn kept_handle(snap: &JobSnapshot) -> &PayloadHandle {
    match &snap.outputs().next().unwrap().payload {
        Payload::Handle(handle) => handle,
        other => panic!("not a handle: {other:?}"),
    }
}

/// The first notice's text; panics if there is none.
fn notice(snap: &JobSnapshot) -> &str {
    snap.events
        .iter()
        .find_map(|e| match e {
            Event::Notice { message } => Some(message.as_str()),
            _ => None,
        })
        .unwrap()
}

/// A failed copy of a signed link keeps the link with its expiry, and the notice says when it
/// stops working, without quoting the link.
#[test]
fn a_failed_copy_says_when_the_link_stops_working() {
    let server = Server::start(|_| http_bytes(403, &[], b"denied"));
    let dir = tempfile::tempdir().unwrap();
    let later = now_unix() + 3600;
    let snap = run(dir.path(), signed(&server, later));
    let kept = kept_handle(&snap);
    assert!(kept.uri.starts_with("http"), "{}", kept.uri);
    assert_eq!(kept.expires_at, Some(later));
    let message = notice(&snap);
    assert!(message.contains("The link stops working at"), "{message}");
    assert!(!message.contains("Signature"), "{message}");
}

/// A failed copy of an unsigned link adds no expiry and no deadline to the notice.
#[test]
fn a_failed_copy_of_an_unsigned_link_adds_no_expiry() {
    let server = Server::start(|_| http_bytes(403, &[], b"denied"));
    let dir = tempfile::tempdir().unwrap();
    let uri = format!("{}/media/out.png", origin(&server));
    let Payload::Handle(handle) = Payload::handle(&uri, "image/png", None).unwrap() else {
        unreachable!()
    };
    let snap = run(dir.path(), handle);
    assert_eq!(kept_handle(&snap).expires_at, None);
    assert!(!notice(&snap).contains("stops working"));
}

/// A link that has already expired says so once, and still carries its expiry.
#[test]
fn an_expired_link_is_reported_once() {
    let server = Server::start(|_| http_bytes(200, &[], PNG));
    let dir = tempfile::tempdir().unwrap();
    let past = now_unix() - CLOCK_GRACE_SECS - 60;
    let snap = run(dir.path(), signed(&server, past));
    let message = notice(&snap);
    assert!(message.contains("expired at"), "{message}");
    assert!(!message.contains("stops working"), "{message}");
    assert_eq!(kept_handle(&snap).expires_at, Some(past));
    assert_eq!(server.count(), 0);
}

/// A copied file carries no expiry: it is on disk now.
#[test]
fn a_saved_copy_has_no_expiry() {
    let server = Server::start(|_| http_bytes(200, &[], PNG));
    let dir = tempfile::tempdir().unwrap();
    let snap = run(dir.path(), signed(&server, now_unix() + 3600));
    let saved = kept_handle(&snap);
    assert!(saved.uri.starts_with("file:"), "{}", saved.uri);
    assert_eq!(saved.expires_at, None);
}

/// A link that redirects to a clearly expired signed link stops there, before asking for it.
#[test]
fn a_redirect_to_an_expired_link_is_not_followed() {
    let past = now_unix() - CLOCK_GRACE_SECS - 60;
    let server = Server::start(move |_| {
        let to = format!("/media/out.png?Expires={past}&Signature=abc");
        http_bytes(302, &[("Location", to.as_str())], b"")
    });
    let dir = tempfile::tempdir().unwrap();
    let uri = format!("{}/media/start.png", origin(&server));
    let Payload::Handle(handle) = Payload::handle(&uri, "image/png", None).unwrap() else {
        unreachable!()
    };
    let error = save(&server, dir.path(), &handle).unwrap_err();
    assert!(error.message.contains("expired at"), "{error:?}");
    assert_eq!(server.count(), 1);
}

/// The earlier deadline wins: a handle that claims to last longer than its signed link is still
/// refused once the link has clearly expired.
#[test]
fn the_earlier_of_two_deadlines_wins() {
    let server = Server::start(|_| http_bytes(200, &[], PNG));
    let dir = tempfile::tempdir().unwrap();
    let mut handle = signed(&server, now_unix() - CLOCK_GRACE_SECS - 60);
    handle.expires_at = Some(now_unix() + 3600);
    let error = save(&server, dir.path(), &handle).unwrap_err();
    assert!(error.message.contains("expired at"), "{error:?}");
    assert_eq!(server.count(), 0);
}

/// Results past the per-poll copy limit stay links, and still carry their deadline.
#[test]
fn a_link_past_the_copy_limit_keeps_its_deadline() {
    let server = Server::start(|_| http_bytes(200, &[], PNG));
    let dir = tempfile::tempdir().unwrap();
    let later = now_unix() + 3600;
    let links = (0..=MAX_SAVES_PER_POLL)
        .map(|i| {
            let uri = format!(
                "{}/media/{i}.png?Expires={later}&Signature=abc",
                origin(&server)
            );
            let Payload::Handle(handle) = Payload::handle(&uri, "image/png", None).unwrap() else {
                unreachable!()
            };
            handle
        })
        .collect();
    let snap = run_all(dir.path(), links);
    let last = match &snap.outputs().last().unwrap().payload {
        Payload::Handle(handle) => handle,
        other => panic!("not a handle: {other:?}"),
    };
    assert!(last.uri.starts_with("http"), "{}", last.uri);
    assert_eq!(last.expires_at, Some(later));
    assert_eq!(server.count(), MAX_SAVES_PER_POLL);
}
