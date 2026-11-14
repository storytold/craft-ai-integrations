//! When a hosted media link stops working, read from the link itself.
//!
//! Delivery hosts sign their links with an expiry in the query string. This module reads the
//! signed forms that most providers' storage uses:
//! - Amazon S3 and anything speaking its signature version 4: `X-Amz-Date` plus `X-Amz-Expires`.
//! - Google Cloud Storage version 4: `X-Goog-Date` plus `X-Goog-Expires`.
//! - S3 and Cloud Storage version 2, and CloudFront: `Expires` as Unix seconds.
//! - Azure shared access signatures: `se` as a UTC date and time.
//!
//! Invariants:
//! - An expiry is read only from a link that also carries that scheme's signature parameter. An
//!   `expires` parameter on an unsigned link is the page's own business, not a deadline.
//! - The earliest expiry wins when a link carries more than one, and when a handle states its own
//!   deadline as well as its link's.
//! - Anything that does not parse is ignored, a date not on the calendar included: no expiry is
//!   better than a wrong one.
//!
//! shortcut: providers that state a lifetime in their docs but sign nothing into the link are
//! not covered here; their adapter can set `PayloadHandle::expires_at` itself when one needs it.

use chrono::{DateTime, NaiveDate, NaiveDateTime};
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

/// A link clearly past its expiry is refused before any request, but only once this much time
/// has passed after it, so a computer whose clock runs a little fast still tries a live link.
pub const CLOCK_GRACE_SECS: u64 = 300;

/// The earlier of a handle's own deadline (`own`) and the one signed into `url`, if either is
/// known.
pub fn earliest(own: Option<u64>, url: &Url) -> Option<u64> {
    match (own, link_expiry(url)) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The Unix time (seconds, UTC) at which a signed link stops working, if the link says.
pub fn link_expiry(url: &Url) -> Option<u64> {
    let mut found: Vec<u64> = Vec::new();
    let param = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.into_owned())
    };
    for (date, lifetime, signature) in [
        ("X-Amz-Date", "X-Amz-Expires", "X-Amz-Signature"),
        ("X-Goog-Date", "X-Goog-Expires", "X-Goog-Signature"),
    ] {
        if param(signature).is_none() {
            continue;
        }
        let start = param(date).and_then(|d| compact_utc(&d));
        let secs = param(lifetime).and_then(|s| s.trim().parse::<u64>().ok());
        if let (Some(start), Some(secs)) = (start, secs) {
            found.push(start.saturating_add(secs));
        }
    }
    if param("Signature").is_some()
        && let Some(at) = param("Expires").and_then(|s| s.trim().parse::<u64>().ok())
    {
        found.push(at);
    }
    if param("sig").is_some()
        && let Some(at) = param("se").and_then(|s| iso_utc(&s))
    {
        found.push(at);
    }
    found.into_iter().min()
}

/// Seconds since the Unix epoch now. A clock set before 1970 reads as zero.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// True when `expires_at` is past by more than [`CLOCK_GRACE_SECS`] at `now`.
pub fn clearly_expired(expires_at: u64, now: u64) -> bool {
    now > expires_at.saturating_add(CLOCK_GRACE_SECS)
}

/// A Unix time as `2026-11-09 12:34 UTC`, for messages. A time past what the calendar can show
/// reads as the bare count of seconds.
pub fn utc_text(unix: u64) -> String {
    i64::try_from(unix)
        .ok()
        .and_then(|secs| DateTime::from_timestamp(secs, 0))
        .map_or_else(
            || format!("{unix} seconds after 1970 (UTC)"),
            |at| at.format("%Y-%m-%d %H:%M UTC").to_string(),
        )
}

/// `20261109T120000Z`, the compact form AWS and Google sign.
fn compact_utc(s: &str) -> Option<u64> {
    let b = s.trim().as_bytes();
    if b.len() != 16 || b[8] != b'T' || b[15] != b'Z' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| digits(&b[r]);
    unix_from(
        n(0..4)?,
        n(4..6)?,
        n(6..8)?,
        n(9..11)?,
        n(11..13)?,
        n(13..15)?,
    )
}

/// `2026-11-09T12:00:00Z`, `2026-11-09T12:00Z` or `2026-11-09`, the forms Azure accepts for `se`.
fn iso_utc(s: &str) -> Option<u64> {
    let b = s.trim().as_bytes();
    let n = |r: std::ops::Range<usize>| digits(b.get(r)?);
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, mo, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    match b.len() {
        10 => unix_from(y, mo, d, 0, 0, 0),
        17 if b[10] == b'T' && b[13] == b':' && b[16] == b'Z' => {
            unix_from(y, mo, d, n(11..13)?, n(14..16)?, 0)
        }
        20 if b[10] == b'T' && b[13] == b':' && b[16] == b':' && b[19] == b'Z' => {
            unix_from(y, mo, d, n(11..13)?, n(14..16)?, n(17..19)?)
        }
        _ => None,
    }
}

/// An HTTP date in the one form hosts must send (`Mon, 09 Nov 2026 12:00:00 GMT`), as Unix
/// seconds. The weekday must match the date. The obsolete forms are not read: a date that does
/// not parse counts as absent.
pub(crate) fn http_date(s: &str) -> Option<u64> {
    let s = s.trim();
    // The fixed length rules out the obsolete forms and a one-digit day.
    if s.len() != 29 {
        return None;
    }
    unix(NaiveDateTime::parse_from_str(s, "%a, %d %b %Y %H:%M:%S GMT").ok()?)
}

/// ASCII digits as a number, or `None` for anything else (an empty slice included).
fn digits(b: &[u8]) -> Option<u64> {
    if b.is_empty() || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(b).ok()?.parse().ok()
}

/// A UTC date and time as Unix seconds, through `chrono`. A field out of range, a day the month
/// does not have, or a time before 1970 give `None`.
fn unix_from(y: u64, mo: u64, d: u64, h: u64, mi: u64, s: u64) -> Option<u64> {
    let n = |v: u64| u32::try_from(v).ok();
    let date = NaiveDate::from_ymd_opt(i32::try_from(y).ok()?, n(mo)?, n(d)?)?;
    unix(date.and_hms_opt(n(h)?, n(mi)?, n(s)?)?)
}

/// A UTC date and time as Unix seconds; `None` before 1970.
fn unix(at: NaiveDateTime) -> Option<u64> {
    u64::try_from(at.and_utc().timestamp()).ok()
}

#[cfg(test)]
#[path = "expiry_tests.rs"]
mod tests;
