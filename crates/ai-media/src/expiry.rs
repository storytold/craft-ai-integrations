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

/// A Unix time as `2026-11-09 12:34 UTC`, for messages.
pub fn utc_text(unix: u64) -> String {
    let days = i64::try_from(unix / 86_400).unwrap_or(i64::MAX);
    let rest = unix % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rest / 3600,
        (rest % 3600) / 60
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
/// seconds. The obsolete forms are not read: a date that does not parse counts as absent.
pub(crate) fn http_date(s: &str) -> Option<u64> {
    const MONTHS: [&[u8]; 12] = [
        b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov",
        b"Dec",
    ];
    let b = s.trim().as_bytes();
    let at = |i: usize, c: u8| b.get(i) == Some(&c);
    if b.len() != 29
        || !(at(3, b',') && at(4, b' ') && at(7, b' ') && at(11, b' ') && at(16, b' '))
        || !(at(19, b':') && at(22, b':') && at(25, b' '))
        || &b[26..] != b"GMT"
    {
        return None;
    }
    let mo = MONTHS.iter().position(|m| *m == &b[8..11])? as u64 + 1;
    let n = |r: std::ops::Range<usize>| digits(&b[r]);
    unix_from(n(12..16)?, mo, n(5..7)?, n(17..19)?, n(20..22)?, n(23..25)?)
}

/// ASCII digits as a number, or `None` for anything else (an empty slice included).
fn digits(b: &[u8]) -> Option<u64> {
    if b.is_empty() || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(b).ok()?.parse().ok()
}

/// A UTC date and time as Unix seconds. Out-of-range fields, a day the month does not have, or
/// a date before 1970 give `None`.
fn unix_from(y: u64, mo: u64, d: u64, h: u64, mi: u64, s: u64) -> Option<u64> {
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    if d > days_in_month(y, mo) {
        return None;
    }
    let days = days_from_civil(i64::try_from(y).ok()?, mo, d);
    let days = u64::try_from(days).ok()?;
    Some(days * 86_400 + h * 3600 + mi * 60 + s)
}

/// Days in month `mo` (1 to 12) of year `y`, leap years included.
fn days_in_month(y: u64, mo: u64) -> u64 {
    match mo {
        2 if y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400)) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: u64, d: u64) -> i64 {
    let (m, d) = (m as i64, d as i64);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date for a count of days since 1970-01-01 (the inverse of [`days_from_civil`]).
fn civil_from_days(z: i64) -> (i64, u64, u64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u64;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u64;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
#[path = "expiry_tests.rs"]
mod tests;
