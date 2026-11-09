//! A download in progress, kept between tries so a cut transfer can carry on where it stopped.
//!
//! Invariants:
//! - A download is kept for resuming only when the host named the file's version: a strong,
//!   well-formed `ETag` (`"..."`), or else a `Last-Modified` that HTTP counts as strong (at least
//!   a second before the response's own `Date`). The next try sends that version as `If-Range`,
//!   so a host whose file has changed sends the whole new file (200) rather than a piece that
//!   does not fit. A weak or malformed `ETag`, and a `Last-Modified` with no later `Date`, may not
//!   be used for `If-Range` and are ignored.
//! - A kept copy belongs to the address its bytes came from, after redirects. A piece from any
//!   other address is not joined on, whatever its version says.
//! - A piece (206) is taken only when its `Content-Range` starts exactly where the copy ends.
//!   Anything else throws the piece and the kept copy away, and the next try starts over.
//! - A body that ends before the end its headers promised is a cut transfer, however cleanly the
//!   connection closed. The copy is published only once it is as long as the file.
//! - The kept copy is the store's temporary file: dropping it deletes it, so a download that
//!   never finishes leaves nothing behind.

use crate::expiry;
use ureq::http::HeaderMap;

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
    if let Some(etag) = text("etag").filter(|e| strong_etag(e)) {
        return Some(etag.to_owned());
    }
    let modified = text("last-modified")?;
    let at = expiry::http_date(modified)?;
    let date = expiry::http_date(text("date")?)?;
    (date >= at.saturating_add(1)).then(|| modified.to_owned())
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

    /// A weak ETag cannot be used for If-Range, so Last-Modified is used, or nothing.
    #[test]
    fn a_weak_etag_is_not_the_version() {
        let h = headers(&[
            ("etag", "W/\"v1\""),
            ("last-modified", "Mon, 09 Nov 2026 12:00:00 GMT"),
            ("date", "Mon, 09 Nov 2026 12:00:01 GMT"),
        ]);
        assert_eq!(
            validator(&h).as_deref(),
            Some("Mon, 09 Nov 2026 12:00:00 GMT")
        );
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

    /// Last-Modified counts only when the response's own Date is at least a second later, as
    /// HTTP requires before a date may be used for If-Range.
    #[test]
    fn last_modified_needs_a_later_date() {
        let modified = ("last-modified", "Mon, 09 Nov 2026 12:00:00 GMT");
        assert_eq!(validator(&headers(&[modified])), None);
        let same = ("date", "Mon, 09 Nov 2026 12:00:00 GMT");
        assert_eq!(validator(&headers(&[modified, same])), None);
        let later = ("date", "Mon, 09 Nov 2026 12:00:01 GMT");
        assert_eq!(
            validator(&headers(&[modified, later])).as_deref(),
            Some("Mon, 09 Nov 2026 12:00:00 GMT")
        );
        let unreadable = ("last-modified", "yesterday");
        assert_eq!(validator(&headers(&[unreadable, later])), None);
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
