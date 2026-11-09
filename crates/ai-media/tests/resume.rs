//! A cut media download carries on where it stopped when the host named the file's version, and
//! starts over when it did not, when the file changed, or when the host's piece does not fit.
//! Scripted localhost servers only.
#![allow(dead_code)]
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_core::{AiError, ErrorKind, Payload, PayloadHandle};
use craft_ai_media::{MediaStore, MediaStoreConfig};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use support::{Recorded, Reply, Server, http_bytes};

/// A picture long enough to cut part way: a PNG signature, then filler.
fn picture() -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    bytes.extend((0..200u8).map(|i| b'a' + i % 26));
    bytes
}

/// The same size, but a different file from its first bytes on: another resource entirely.
fn other_picture() -> Vec<u8> {
    let mut bytes = picture();
    bytes[20] = b'Q';
    bytes
}

/// The same size, but a different file: the host's newer version.
fn newer_picture() -> Vec<u8> {
    let mut bytes = picture();
    let last = bytes.len() - 1;
    bytes[last] = b'Z';
    bytes
}

const CUT: usize = 40;

/// A store config in `dir` that may reach localhost and does not wait between tries.
fn config(dir: &Path) -> MediaStoreConfig {
    let mut cfg = MediaStoreConfig::new(dir);
    cfg.allow_loopback = true;
    cfg.retry_delay = Duration::ZERO;
    cfg
}

/// Saves the server's `/media/out.png` through a fresh store in `dir`.
fn save(server: &Server, dir: &Path) -> Result<PayloadHandle, AiError> {
    save_with(server, config(dir))
}

/// Saves the server's `/media/out.png` through a fresh store with `cfg`.
fn save_with(server: &Server, cfg: MediaStoreConfig) -> Result<PayloadHandle, AiError> {
    let uri = format!("{}/media/out.png", server.url.trim_end_matches("/v1"));
    let Payload::Handle(handle) = Payload::handle(&uri, "image/png", None).unwrap() else {
        unreachable!()
    };
    MediaStore::new(cfg).unwrap().save(&handle)
}

/// A redirect to `path` on the same server.
fn moved(path: &'static str) -> Answer {
    Box::new(move |_| http_bytes(302, &[("Location", path)], b""))
}

/// The bytes of the local file a saved handle points at.
fn saved_bytes(handle: &PayloadHandle) -> Vec<u8> {
    let path: PathBuf = url::Url::parse(&handle.uri)
        .unwrap()
        .to_file_path()
        .unwrap();
    std::fs::read(path).unwrap()
}

/// A 200 that promises the whole picture, sends the first `CUT` bytes and closes.
fn cut_whole(extra_headers: &str) -> Reply {
    let full = picture();
    let mut raw = format!(
        "HTTP/1.1 200 X\r\nContent-Length: {}\r\n{extra_headers}Connection: close\r\n\r\n",
        full.len()
    )
    .into_bytes();
    raw.extend_from_slice(&full[..CUT]);
    Reply::Raw(raw)
}

/// A 206 carrying `bytes[start..]` of `full`, labelled as starting at `label_start`.
fn piece(full: &[u8], start: usize, label_start: usize) -> Reply {
    let range = format!(
        "bytes {label_start}-{}/{}",
        label_start + full.len() - start - 1,
        full.len()
    );
    http_bytes(206, &[("Content-Range", range.as_str())], &full[start..])
}

/// One scripted answer: the reply to send for a recorded request.
type Answer = Box<dyn Fn(&Recorded) -> Reply + Send + Sync>;

/// A server that answers each request with the next scripted reply, recording what it was sent.
fn scripted(replies: Vec<Answer>) -> Server {
    let next = Arc::new(AtomicUsize::new(0));
    Server::start(move |request| {
        let i = next.fetch_add(1, Ordering::SeqCst);
        replies[i.min(replies.len() - 1)](request)
    })
}

/// The `name` header of the server's `i`th request, if it was sent.
fn sent(server: &Server, i: usize, name: &str) -> Option<String> {
    server.requests.lock().unwrap()[i]
        .header(name)
        .map(str::to_owned)
}

/// With a strong ETag, the second try asks for the rest against that version and the pieces join.
#[test]
fn a_cut_download_with_an_etag_carries_on() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| cut_whole("ETag: \"v1\"\r\n")),
        Box::new(|_| piece(&picture(), CUT, CUT)),
    ]);
    let saved = save(&server, dir.path()).unwrap();
    assert_eq!(saved_bytes(&saved), picture());
    assert_eq!(saved.size_bytes, Some(picture().len() as u64));
    assert_eq!(server.count(), 2);
    assert_eq!(sent(&server, 0, "range"), None);
    assert_eq!(sent(&server, 1, "range").as_deref(), Some("bytes=40-"));
    assert_eq!(sent(&server, 1, "if-range").as_deref(), Some("\"v1\""));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

/// Last-Modified serves as the version when there is no strong ETag.
#[test]
fn last_modified_serves_as_the_version() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| {
            cut_whole(
                "ETag: W/\"weak\"\r\nLast-Modified: Mon, 09 Nov 2026 12:00:00 GMT\r\n\
                 Date: Mon, 09 Nov 2026 12:00:05 GMT\r\n",
            )
        }),
        Box::new(|_| piece(&picture(), CUT, CUT)),
    ]);
    let saved = save(&server, dir.path()).unwrap();
    assert_eq!(saved_bytes(&saved), picture());
    assert_eq!(
        sent(&server, 1, "if-range").as_deref(),
        Some("Mon, 09 Nov 2026 12:00:00 GMT")
    );
}

/// Without a version the second try starts over and asks for no range.
#[test]
fn a_cut_download_without_a_version_starts_over() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| cut_whole("")),
        Box::new(|_| http_bytes(200, &[], &picture())),
    ]);
    let saved = save(&server, dir.path()).unwrap();
    assert_eq!(saved_bytes(&saved), picture());
    assert_eq!(sent(&server, 1, "range"), None);
    assert_eq!(sent(&server, 1, "if-range"), None);
}

/// A host whose file changed answers the range with the whole new file, which replaces the copy.
#[test]
fn a_changed_file_replaces_the_kept_copy() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| cut_whole("ETag: \"v1\"\r\n")),
        Box::new(|_| http_bytes(200, &[("ETag", "\"v2\"")], &newer_picture())),
    ]);
    let saved = save(&server, dir.path()).unwrap();
    assert_eq!(saved_bytes(&saved), newer_picture());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

/// A piece that starts anywhere but the end of the copy is thrown away, and the next try starts
/// over from the first byte.
#[test]
fn a_piece_that_does_not_fit_starts_over() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| cut_whole("ETag: \"v1\"\r\n")),
        Box::new(|_| piece(&picture(), CUT - 10, CUT - 10)),
        Box::new(|_| http_bytes(200, &[], &picture())),
    ]);
    let saved = save(&server, dir.path()).unwrap();
    assert_eq!(saved_bytes(&saved), picture());
    assert_eq!(server.count(), 3);
    assert_eq!(sent(&server, 2, "range"), None);
}

/// A 416 for the rest drops the copy and the next try starts over.
#[test]
fn a_refused_range_starts_over() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| cut_whole("ETag: \"v1\"\r\n")),
        Box::new(|_| http_bytes(416, &[], b"")),
        Box::new(|_| http_bytes(200, &[], &picture())),
    ]);
    let saved = save(&server, dir.path()).unwrap();
    assert_eq!(saved_bytes(&saved), picture());
    assert_eq!(sent(&server, 2, "range"), None);
}

/// A piece whose total is over the size limit is refused, not appended.
#[test]
fn a_piece_over_the_limit_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| cut_whole("ETag: \"v1\"\r\n")),
        Box::new(|_| {
            http_bytes(
                206,
                &[("Content-Range", "bytes 40-99/9999999999999")],
                &[b'x'; 60],
            )
        }),
    ]);
    let error = save(&server, dir.path()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Provider);
    assert!(error.message.contains("larger than"), "{error:?}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

/// A download cut on every try leaves no part-saved file once the store gives up.
#[test]
fn a_download_that_never_finishes_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| cut_whole("ETag: \"v1\"\r\n")),
        Box::new(|_| {
            let raw = b"HTTP/1.1 206 X\r\nContent-Range: bytes 40-215/216\r\nContent-Length: 176\r\nConnection: close\r\n\r\nabc".to_vec();
            Reply::Raw(raw)
        }),
    ]);
    let error = save(&server, dir.path()).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Network);
    assert!(error.message.contains("after 3 tries"), "{error:?}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    // Each later try asked for the rest from where the copy had got to.
    assert_eq!(sent(&server, 1, "range").as_deref(), Some("bytes=40-"));
    assert_eq!(sent(&server, 2, "range").as_deref(), Some("bytes=43-"));
}

/// A piece that ends before the end its range promised, however cleanly it closes, is not
/// published: the next try asks for the rest and the file is whole.
#[test]
fn a_short_piece_is_not_published() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| cut_whole("ETag: \"v1\"\r\n")),
        Box::new(|_| {
            let full = picture();
            let range = format!("bytes {CUT}-{}/{}", full.len() - 1, full.len());
            http_bytes(206, &[("Content-Range", range.as_str())], &full[CUT..100])
        }),
        Box::new(|_| piece(&picture(), 100, 100)),
    ]);
    let saved = save(&server, dir.path()).unwrap();
    assert_eq!(saved_bytes(&saved), picture());
    assert_eq!(server.count(), 3);
    assert_eq!(sent(&server, 2, "range").as_deref(), Some("bytes=100-"));
}

/// A whole-file answer shorter than its Content-Length is not published either.
#[test]
fn a_short_whole_file_is_not_published() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| {
            let full = picture();
            let mut raw = format!(
                "HTTP/1.1 200 X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                full.len()
            )
            .into_bytes();
            raw.extend_from_slice(&full[..CUT]);
            Reply::Raw(raw)
        }),
        Box::new(|_| http_bytes(200, &[], &picture())),
    ]);
    let saved = save(&server, dir.path()).unwrap();
    assert_eq!(saved_bytes(&saved), picture());
    assert_eq!(server.count(), 2);
}

/// A kept copy belongs to the address its bytes came from. When the link later leads somewhere
/// else, a piece from there is not joined on, even with the same version, and the next try
/// starts over.
#[test]
fn a_piece_from_another_address_starts_over() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        moved("/media/a.png"),
        Box::new(|_| cut_whole("ETag: \"v1\"\r\n")),
        moved("/media/b.png"),
        Box::new(|_| piece(&other_picture(), CUT, CUT)),
        moved("/media/b.png"),
        Box::new(|_| http_bytes(200, &[], &other_picture())),
    ]);
    let saved = save(&server, dir.path()).unwrap();
    assert_eq!(saved_bytes(&saved), other_picture());
    assert_eq!(server.count(), 6);
    assert_eq!(sent(&server, 4, "range"), None);
}

/// The retry budget counts pauses, not transfer time: with no budget and no pause, a cut
/// download still carries on.
#[test]
fn time_spent_downloading_does_not_use_the_retry_budget() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| cut_whole("ETag: \"v1\"\r\n")),
        Box::new(|_| piece(&picture(), CUT, CUT)),
    ]);
    let mut cfg = config(dir.path());
    cfg.retry_budget = Duration::ZERO;
    let saved = save_with(&server, cfg).unwrap();
    assert_eq!(saved_bytes(&saved), picture());
    assert_eq!(server.count(), 2);
}

/// A piece longer than its own range is not joined on: the next try starts over.
#[test]
fn a_piece_longer_than_its_range_starts_over() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| cut_whole("ETag: \"v1\"\r\n")),
        Box::new(|_| {
            let full = picture();
            let range = format!("bytes {CUT}-49/{}", full.len());
            http_bytes(
                206,
                &[("Content-Range", range.as_str())],
                &other_picture()[CUT..100],
            )
        }),
        Box::new(|_| http_bytes(200, &[], &picture())),
    ]);
    let saved = save(&server, dir.path()).unwrap();
    assert_eq!(saved_bytes(&saved), picture());
    assert_eq!(server.count(), 3);
    assert_eq!(sent(&server, 2, "range"), None);
}

/// A piece that starts past the end of the copy would leave a gap, so it is thrown away and the
/// next try starts over.
#[test]
fn a_piece_that_skips_ahead_starts_over() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| {
            cut_whole(
                "ETag: \"v1\"
",
            )
        }),
        Box::new(|_| piece(&picture(), CUT + 10, CUT + 10)),
        Box::new(|_| http_bytes(200, &[], &picture())),
    ]);
    let saved = save(&server, dir.path()).unwrap();
    assert_eq!(saved_bytes(&saved), picture());
    assert_eq!(server.count(), 3);
    assert_eq!(sent(&server, 2, "range"), None);
}
