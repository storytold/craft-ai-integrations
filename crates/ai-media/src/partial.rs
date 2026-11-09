//! A download in progress, kept between tries so a cut transfer can carry on where it stopped.
//!
//! Invariants:
//! - A download is kept for resuming only when the host named the file's version: a strong,
//!   well-formed `ETag` (`"..."`), or, when the answer carries no `ETag` at all, a
//!   `Last-Modified` that a client may count as strong: at least 60 seconds before the response's
//!   own `Date` (RFC 9110, 8.8.2.2 and 13.1.5). The next try sends that version as `If-Range`, so
//!   a host whose file has changed sends the whole new file (200) rather than a piece that does
//!   not fit. A weak or malformed `ETag` means no version at all: it may not be sent, and it
//!   rules out falling back to the date.
//! - A kept copy belongs to the address its bytes came from, after redirects. A piece from any
//!   other address is not joined on, whatever its version says.
//! - A piece (206) is taken only when its `Content-Range` starts exactly where the copy ends, it
//!   names no other version than the kept one, and its total agrees with any total already
//!   known. Anything else throws the piece and the kept copy away, and the next try starts over.
//! - The whole file's length, once any answer has said it, is kept with the copy, so a later
//!   piece that leaves its total unsaid (`*`) cannot end the file early.
//! - A body that ends before the end its headers promised is a cut transfer, however cleanly the
//!   connection closed. The copy is published only once it is as long as the file.
//! - The kept copy is the store's temporary file: dropping it deletes it, so a download that
//!   never finishes leaves nothing behind.

use crate::expiry;
use ureq::http::HeaderMap;

/// How long before the response's own `Date` a `Last-Modified` must be for a client to count it
/// as a strong validator, allowing for clocks that disagree (RFC 9110, 8.8.2.2).
const STRONG_DATE_GAP_SECS: u64 = 60;

/// A part-saved copy and what is needed to ask for the rest.
pub(crate) struct Partial {
    pub temp: tempfile::NamedTempFile,
    /// Bytes already in `temp`.
    pub written: u64,
    /// The `If-Range` value: a strong ETag, or a `Last-Modified` date. Always present for a
    /// kept download, so a download without one is never resumed.
    pub validator: String,
    /// The media type read from the first bytes, kept because a piece has no first bytes.
    pub media: &'static str,
    /// The address the kept bytes came from, after redirects.
    pub source: String,
    /// The whole file's length, once any answer has said it.
    pub whole: Option<u64>,
}

/// The version a host gave the file, fit for `If-Range`: a strong ETag, else a strong
/// `Last-Modified` (see the module notes).
pub(crate) fn validator(headers: &HeaderMap) -> Option<String> {
    let text = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    // A date may be sent only when there is no entity tag at all (RFC 9110, 13.1.5).
    if let Some(etag) = text("etag") {
        return strong_etag(etag).then(|| etag.to_owned());
    }
    let modified = text("last-modified")?;
    let at = expiry::http_date(modified)?;
    let date = expiry::http_date(text("date")?)?;
    (date >= at.saturating_add(STRONG_DATE_GAP_SECS)).then(|| modified.to_owned())
}

/// True when a piece's headers name a version other than `kept`: a different `ETag` for a kept
/// `ETag`, or a different `Last-Modified` for a kept date. A piece that names none is not
/// contradicting.
pub(crate) fn contradicts(kept: &str, headers: &HeaderMap) -> bool {
    let named = if kept.starts_with('"') {
        "etag"
    } else {
        "last-modified"
    };
    headers
        .get(named)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .is_some_and(|v| v != kept)
}

/// True for a strong entity tag as HTTP writes it: a quoted string with no quote inside and no
/// `W/` in front.
fn strong_etag(etag: &str) -> bool {
    let b = etag.as_bytes();
    b.len() >= 2
        && b[0] == b'"'
        && b[b.len() - 1] == b'"'
        && b[1..b.len() - 1]
            .iter()
            .all(|&c| c == 0x21 || (0x23..=0x7e).contains(&c) || c >= 0x80)
}

/// What a `Content-Range` says about a piece: where it starts and ends (inclusive), and the whole
/// file's size if the host knows it. `None` when it does not parse as `bytes START-END/TOTAL`
/// (TOTAL may be `*`).
pub(crate) fn content_range(headers: &HeaderMap) -> Option<(u64, u64, Option<u64>)> {
    let value = headers.get("content-range")?.to_str().ok()?.trim();
    let rest = value.strip_prefix("bytes ")?;
    let (span, total) = rest.split_once('/')?;
    let (start, end) = span.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    let end: u64 = end.trim().parse().ok()?;
    if end < start {
        return None;
    }
    let total = match total.trim() {
        "*" => None,
        n => Some(n.parse::<u64>().ok().filter(|t| *t > end)?),
    };
    Some((start, end, total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ureq::http::HeaderValue;

    /// A header map holding each `(name, value)` pair.
    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    }

    /// A strong ETag wins over Last-Modified.
    #[test]
    fn a_strong_etag_is_the_version() {
        let h = headers(&[
            ("etag", "\"v1\""),
            ("last-modified", "Mon, 09 Nov 2026 12:00:00 GMT"),
        ]);
        assert_eq!(validator(&h).as_deref(), Some("\"v1\""));
    }

    /// A weak ETag cannot be used for If-Range, and while there is one the date may not be used
    /// either, so there is no version.
    #[test]
    fn a_weak_etag_is_not_the_version() {
        let h = headers(&[
            ("etag", "W/\"v1\""),
            ("last-modified", "Mon, 09 Nov 2026 12:00:00 GMT"),
            ("date", "Mon, 09 Nov 2026 12:05:00 GMT"),
        ]);
        assert_eq!(validator(&h), None);
        assert_eq!(validator(&headers(&[("etag", "W/\"v1\"")])), None);
        assert_eq!(validator(&headers(&[])), None);
    }

    /// An ETag that is not a quoted string is not a version, so it is never sent back.
    #[test]
    fn a_malformed_etag_is_not_the_version() {
        for bad in ["v1", "v1\"", "\"v1", "\"v\"1\"", "\""] {
            assert_eq!(validator(&headers(&[("etag", bad)])), None, "{bad}");
        }
    }

    /// Last-Modified counts only when the response's own Date is at least 60 seconds later, as
    /// HTTP requires before a client may use a date for If-Range.
    #[test]
    fn last_modified_needs_a_later_date() {
        let modified = ("last-modified", "Mon, 09 Nov 2026 12:00:00 GMT");
        assert_eq!(validator(&headers(&[modified])), None);
        let same = ("date", "Mon, 09 Nov 2026 12:00:00 GMT");
        assert_eq!(validator(&headers(&[modified, same])), None);
        let close = ("date", "Mon, 09 Nov 2026 12:00:59 GMT");
        assert_eq!(validator(&headers(&[modified, close])), None);
        let later = ("date", "Mon, 09 Nov 2026 12:01:00 GMT");
        assert_eq!(
            validator(&headers(&[modified, later])).as_deref(),
            Some("Mon, 09 Nov 2026 12:00:00 GMT")
        );
        let unreadable = ("last-modified", "yesterday");
        assert_eq!(validator(&headers(&[unreadable, later])), None);
    }

    /// A piece contradicts the kept copy only when it names a different version of the same kind.
    #[test]
    fn a_piece_contradicts_only_with_another_version() {
        let etag = "\"v1\"";
        assert!(!contradicts(etag, &headers(&[("etag", "\"v1\"")])));
        assert!(contradicts(etag, &headers(&[("etag", "\"v2\"")])));
        assert!(!contradicts(etag, &headers(&[])));
        let date = "Mon, 09 Nov 2026 12:00:00 GMT";
        assert!(!contradicts(date, &headers(&[("last-modified", date)])));
        let later = ("last-modified", "Mon, 09 Nov 2026 13:00:00 GMT");
        assert!(contradicts(date, &headers(&[later])));
        assert!(!contradicts(date, &headers(&[("etag", "\"v1\"")])));
    }

    /// The usual forms of Content-Range.
    #[test]
    fn content_ranges_that_parse() {
        let h = |v| headers(&[("content-range", v)]);
        assert_eq!(
            content_range(&h("bytes 20-99/100")),
            Some((20, 99, Some(100)))
        );
        assert_eq!(content_range(&h("bytes 20-99/*")), Some((20, 99, None)));
    }

    /// Malformed or impossible ranges are refused.
    #[test]
    fn content_ranges_that_do_not() {
        let h = |v| headers(&[("content-range", v)]);
        for bad in [
            "bytes */100",
            "items 0-1/2",
            "bytes 9-1/100",
            "bytes 0-99/99",
            "bytes 0-9/x",
        ] {
            assert_eq!(content_range(&h(bad)), None, "{bad}");
        }
        assert_eq!(content_range(&headers(&[])), None);
    }
}
