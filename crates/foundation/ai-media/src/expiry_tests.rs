use super::*;

/// 2026-11-09 12:00:00 UTC.
const NOON: u64 = 1_794_225_600;

/// The expiry signed into `link`, which must parse as a URL.
fn expiry(link: &str) -> Option<u64> {
    link_expiry(&Url::parse(link).unwrap())
}

/// The epoch, a leap day and the reference noon convert both ways.
#[test]
fn dates_convert_both_ways() {
    assert_eq!(unix_from(1970, 1, 1, 0, 0, 0), Some(0));
    assert_eq!(unix_from(2026, 11, 9, 12, 0, 0), Some(NOON));
    assert_eq!(utc_text(NOON), "2026-11-09 12:00 UTC");
    let leap = unix_from(2028, 2, 29, 23, 59, 0).unwrap();
    assert_eq!(utc_text(leap), "2028-02-29 23:59 UTC");
    assert_eq!(utc_text(0), "1970-01-01 00:00 UTC");
}

/// Impossible fields and dates before 1970 are not read.
#[test]
fn bad_dates_are_ignored() {
    assert_eq!(unix_from(2026, 13, 1, 0, 0, 0), None);
    assert_eq!(unix_from(2026, 1, 1, 24, 0, 0), None);
    assert_eq!(unix_from(1969, 12, 31, 0, 0, 0), None);
    assert_eq!(compact_utc("2026-11-09T12:00:00Z"), None);
    assert_eq!(iso_utc("2026/11/09"), None);
    assert_eq!(digits(b""), None);
    assert_eq!(digits(b"1a"), None);
}

/// AWS signature version 4: the signing time plus the lifetime.
#[test]
fn aws_v4() {
    let link = "https://bucket.s3.amazonaws.com/out.png?X-Amz-Date=20261109T120000Z&X-Amz-Expires=3600&X-Amz-Signature=abc";
    assert_eq!(expiry(link), Some(NOON + 3600));
}

/// Google Cloud Storage version 4, with lower-case names.
#[test]
fn google_v4() {
    let link = "https://storage.googleapis.com/b/o.png?x-goog-date=20261109T120000Z&x-goog-expires=600&x-goog-signature=abc";
    assert_eq!(expiry(link), Some(NOON + 600));
}

/// Version 2 and CloudFront carry Unix seconds in `Expires`.
#[test]
fn unix_expires() {
    let link = format!("https://cdn.example.com/o.mp4?Expires={NOON}&Signature=abc&Key-Pair-Id=K");
    assert_eq!(expiry(&link), Some(NOON));
}

/// Azure shared access signatures, in each form Azure accepts.
#[test]
fn azure_sas() {
    for se in ["2026-11-09T12:00:00Z", "2026-11-09T12:00Z"] {
        let link = format!("https://a.blob.core.windows.net/c/o.png?sv=2024&se={se}&sig=abc");
        assert_eq!(expiry(&link), Some(NOON), "{se}");
    }
    let link = "https://a.blob.core.windows.net/c/o.png?se=2026-11-09&sig=abc";
    assert_eq!(expiry(link), Some(NOON - 12 * 3600));
}

/// An unsigned link's `expires` parameter is not a deadline.
#[test]
fn unsigned_links_have_no_expiry() {
    assert_eq!(expiry(&format!("https://e.com/o.png?Expires={NOON}")), None);
    assert_eq!(expiry("https://e.com/o.png?se=2026-11-09T12:00:00Z"), None);
    assert_eq!(
        expiry("https://e.com/o.png?X-Amz-Date=20261109T120000Z&X-Amz-Expires=60"),
        None
    );
    assert_eq!(expiry("https://e.com/o.png"), None);
}

/// A signed link whose dates do not parse has no expiry rather than a wrong one.
#[test]
fn unreadable_expiry_is_ignored() {
    assert_eq!(
        expiry("https://e.com/o.png?X-Amz-Date=yesterday&X-Amz-Expires=60&X-Amz-Signature=a"),
        None
    );
    assert_eq!(expiry("https://e.com/o.png?Expires=soon&Signature=a"), None);
}

/// With two schemes on one link, the earlier expiry wins.
#[test]
fn the_earliest_expiry_wins() {
    let link = format!(
        "https://e.com/o.png?X-Amz-Date=20261109T120000Z&X-Amz-Expires=3600&X-Amz-Signature=a&Expires={}&Signature=b",
        NOON + 60
    );
    assert_eq!(expiry(&link), Some(NOON + 60));
}

/// A link is clearly expired only once the grace has passed after its expiry.
#[test]
fn grace_allows_a_fast_clock() {
    assert!(!clearly_expired(NOON, NOON));
    assert!(!clearly_expired(NOON, NOON + CLOCK_GRACE_SECS));
    assert!(clearly_expired(NOON, NOON + CLOCK_GRACE_SECS + 1));
    assert!(!clearly_expired(u64::MAX, u64::MAX));
}

/// A date that is not on the calendar gives no expiry rather than a made-up one.
#[test]
fn a_date_not_on_the_calendar_is_ignored() {
    assert_eq!(
        expiry("https://a.blob.core.windows.net/c/b.png?se=2026-02-31&sig=abc"),
        None
    );
    assert_eq!(unix_from(2026, 2, 29, 0, 0, 0), None);
    assert_eq!(unix_from(2026, 4, 31, 0, 0, 0), None);
    assert_eq!(unix_from(2100, 2, 29, 0, 0, 0), None);
    assert!(unix_from(2028, 2, 29, 0, 0, 0).is_some());
    assert!(unix_from(2000, 2, 29, 0, 0, 0).is_some());
}

/// An HTTP date is read in its one standard form only, with a weekday that matches the date.
#[test]
fn http_dates_are_read_in_the_standard_form() {
    assert_eq!(http_date("Mon, 09 Nov 2026 12:00:00 GMT"), Some(NOON));
    assert_eq!(http_date(" Mon, 09 Nov 2026 12:00:01 GMT "), Some(NOON + 1));
    for bad in [
        "Monday, 09-Nov-26 12:00:00 GMT",
        "Mon Nov  9 12:00:00 2026",
        "Mon, 09 Nox 2026 12:00:00 GMT",
        "Mon, 31 Nov 2026 12:00:00 GMT",
        "Mon, 09 Nov 2026 12:00:00 UTC",
        "Tue, 09 Nov 2026 12:00:00 GMT",
        "Xyz, 09 Nov 2026 12:00:00 GMT",
    ] {
        assert_eq!(http_date(bad), None, "{bad}");
    }
}

/// The earlier of a handle's own deadline and its link's is the one kept.
#[test]
fn the_earliest_deadline_is_kept() {
    let signed = Url::parse(&format!("https://h.example/x?Expires={NOON}&Signature=a")).unwrap();
    let plain = Url::parse("https://h.example/x").unwrap();
    assert_eq!(earliest(Some(NOON + 10), &signed), Some(NOON));
    assert_eq!(earliest(Some(NOON - 10), &signed), Some(NOON - 10));
    assert_eq!(earliest(None, &signed), Some(NOON));
    assert_eq!(earliest(Some(5), &plain), Some(5));
    assert_eq!(earliest(None, &plain), None);
}
