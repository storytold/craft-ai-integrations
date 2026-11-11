//! A cut media download carries on where it stopped when the host named the file's version, and
//! starts over when it did not, when the file changed, or when the host's piece does not fit.
//! Scripted localhost servers only.
#![allow(dead_code)]
mod resume_support;
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;

use craft_ai_core::ErrorKind;
use resume_support::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use support::{Reply, http_bytes};

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

/// Last-Modified serves as the version when there is no ETag.
#[test]
fn last_modified_serves_as_the_version() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| {
            cut_whole(
                "Last-Modified: Mon, 09 Nov 2026 12:00:00 GMT\r\n\
                 Date: Mon, 09 Nov 2026 12:05:00 GMT\r\n",
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
    let folder = dir.path().to_path_buf();
    let between = Arc::new(AtomicUsize::new(usize::MAX));
    let seen = Arc::clone(&between);
    let server = scripted(vec![
        Box::new(|_| cut_whole("")),
        Box::new(move |_| {
            // Nothing is kept between tries: the cut copy was deleted.
            seen.store(
                std::fs::read_dir(&folder).unwrap().count(),
                Ordering::SeqCst,
            );
            http_bytes(200, &[], &picture())
        }),
    ]);
    let saved = save(&server, dir.path()).unwrap();
    assert_eq!(between.load(Ordering::SeqCst), 0);
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

/// A 416 for the rest drops the copy, and the whole file is asked for again.
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
    cfg.retry.budget = Duration::ZERO;
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

/// A total learned from the first answer still counts when a later piece leaves it unsaid, so
/// the copy is not published short.
#[test]
fn a_lost_total_is_still_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| cut_whole("ETag: \"v1\"\r\n")),
        Box::new(|_| {
            let full = picture();
            let range = format!("bytes {CUT}-99/*");
            http_bytes(206, &[("Content-Range", range.as_str())], &full[CUT..100])
        }),
        Box::new(|_| piece(&picture(), 100, 100)),
    ]);
    let saved = save(&server, dir.path()).unwrap();
    assert_eq!(saved_bytes(&saved), picture());
    assert_eq!(server.count(), 3);
    assert_eq!(sent(&server, 2, "range").as_deref(), Some("bytes=100-"));
}

/// A piece whose total disagrees with the one already known is another file: start over.
#[test]
fn a_different_total_starts_over() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| cut_whole("ETag: \"v1\"\r\n")),
        Box::new(|_| {
            let full = picture();
            let range = format!("bytes {CUT}-{}/{}", full.len() - 1, full.len() + 50);
            http_bytes(206, &[("Content-Range", range.as_str())], &full[CUT..])
        }),
        Box::new(|_| http_bytes(200, &[], &picture())),
    ]);
    let saved = save(&server, dir.path()).unwrap();
    assert_eq!(saved_bytes(&saved), picture());
    assert_eq!(server.count(), 3);
    assert_eq!(sent(&server, 2, "range"), None);
}

/// A piece that names another version is not joined on, even when it fits: start over.
#[test]
fn a_piece_of_another_version_starts_over() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| cut_whole("ETag: \"v1\"\r\n")),
        Box::new(|_| {
            let full = other_picture();
            let range = format!("bytes {CUT}-{}/{}", full.len() - 1, full.len());
            let headers = [("Content-Range", range.as_str()), ("ETag", "\"v2\"")];
            http_bytes(206, &headers, &full[CUT..])
        }),
        Box::new(|_| http_bytes(200, &[], &picture())),
    ]);
    let saved = save(&server, dir.path()).unwrap();
    assert_eq!(saved_bytes(&saved), picture());
    assert_eq!(server.count(), 3);
    assert_eq!(sent(&server, 2, "range"), None);
}

/// A cut piece that had already sent more than its range allowed is not kept for the next try.
#[test]
fn an_overlong_cut_piece_is_not_kept() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| cut_whole("ETag: \"v1\"\r\n")),
        Box::new(|_| {
            let full = picture();
            let mut raw = format!(
                "HTTP/1.1 206 X\r\nContent-Range: bytes {CUT}-49/{}\r\nContent-Length: 176\r\n\
                 Connection: close\r\n\r\n",
                full.len()
            )
            .into_bytes();
            raw.extend_from_slice(&full[CUT..100]);
            Reply::Raw(raw)
        }),
        Box::new(|_| http_bytes(200, &[], &picture())),
    ]);
    let saved = save(&server, dir.path()).unwrap();
    assert_eq!(saved_bytes(&saved), picture());
    assert_eq!(server.count(), 3);
    assert_eq!(sent(&server, 2, "range"), None);
}

/// A link that redirects to a freshly signed address on every request (a new signature in the
/// query) still carries on: the copy belongs to the path, and the version keeps files apart.
#[test]
fn a_freshly_signed_redirect_still_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        moved("/media/a.png?X-Amz-Signature=one"),
        Box::new(|_| cut_whole("ETag: \"v1\"\r\n")),
        moved("/media/a.png?X-Amz-Signature=two"),
        Box::new(|_| piece(&picture(), CUT, CUT)),
    ]);
    let saved = save(&server, dir.path()).unwrap();
    assert_eq!(saved_bytes(&saved), picture());
    assert_eq!(server.count(), 4);
    assert_eq!(sent(&server, 3, "range").as_deref(), Some("bytes=40-"));
}

/// When the host cannot send the rest (416), the whole file is asked for at once in the same
/// try: with two tries, one real cut and one refused range still end in a saved file.
#[test]
fn a_refused_range_does_not_cost_a_try() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| cut_whole("ETag: \"v1\"\r\n")),
        Box::new(|_| http_bytes(416, &[], b"")),
        Box::new(|_| http_bytes(200, &[], &picture())),
    ]);
    let mut cfg = config(dir.path());
    cfg.retry.max_attempts = 2;
    let saved = save_with(&server, cfg).unwrap();
    assert_eq!(saved_bytes(&saved), picture());
    assert_eq!(server.count(), 3);
    assert_eq!(sent(&server, 2, "range"), None);
}

/// The time limit covers the whole save, not each try: the second try gets only what is left,
/// and no further pause or try follows once it has run out.
#[test]
fn the_whole_save_has_one_time_limit() {
    let dir = tempfile::tempdir().unwrap();
    let server = scripted(vec![
        Box::new(|_| {
            std::thread::sleep(Duration::from_millis(200));
            cut_whole("")
        }),
        Box::new(|_| {
            std::thread::sleep(Duration::from_millis(800));
            http_bytes(200, &[], &picture())
        }),
    ]);
    let mut cfg = config(dir.path());
    cfg.transfer_timeout = Duration::from_secs(1);
    cfg.retry.first_delay = Duration::from_millis(300);
    let started = std::time::Instant::now();
    let error = save_with(&server, cfg).unwrap_err();
    let took = started.elapsed();
    assert!(error.message.contains("time limit"), "{error:?}");
    assert!(took < Duration::from_millis(1400), "{took:?}");
    assert_eq!(server.count(), 2);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
