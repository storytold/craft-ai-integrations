//! A check every JPEG decoder in the workspace runs before it hands a file to `image`/`zune-jpeg`.
//!
//! `zune-jpeg` re-sorts and re-scans every Extended XMP fragment collected so far after each
//! header segment, so a file made of thousands of tiny fragments costs it quadratic time before
//! any size limit applies. A real file needs a handful; [`MAX_XMP_FRAGMENTS`] fragments hold
//! 64 MB of XMP. Invariant: a decoder that calls [`too_many_xmp_fragments`] first and refuses on
//! `true` never passes the decoder more than that many fragments.
//!
//! The count is one linear pass over the header segments that stops at the first scan, so its
//! own cost is bounded by the file's length. It reads only bytes, with no I/O, so it compiles for
//! wasm with the rest of this crate.

/// The most Extended XMP fragments (APP1 segments) a JPEG may carry before it is refused.
pub const MAX_XMP_FRAGMENTS: usize = 1024;

/// The namespace that opens every Extended XMP fragment's APP1 body.
pub const EXTENDED_XMP_PREFIX: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";

/// Counts the Extended XMP fragments among a JPEG's header segments, in one pass that stops at
/// the first scan. Anything malformed ends the count and is left for the decoder to refuse.
pub fn extended_xmp_fragments(data: &[u8]) -> usize {
    let mut count = 0;
    let mut at = 2;
    while let Some(&byte) = data.get(at) {
        if byte != 0xFF {
            break;
        }
        at += 1;
        match data.get(at) {
            // Fill bytes, and markers that stand alone, carry no length.
            Some(0xFF) => continue,
            Some(0x01 | 0xD0..=0xD9) => at += 1,
            Some(0xDA) | None => break,
            Some(&marker) => {
                let Some(length) = data.get(at + 1..at + 3) else {
                    break;
                };
                let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
                let body = data.get(at + 3..(at + 1 + length).min(data.len()));
                if marker == 0xE1 && body.is_some_and(|b| b.starts_with(EXTENDED_XMP_PREFIX)) {
                    count += 1;
                }
                at += 1 + length.max(2);
            }
        }
    }
    count
}

/// Whether a JPEG carries more than [`MAX_XMP_FRAGMENTS`] Extended XMP fragments, and so must be
/// refused before it reaches a decoder.
pub fn too_many_xmp_fragments(data: &[u8]) -> bool {
    extended_xmp_fragments(data) > MAX_XMP_FRAGMENTS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One APP1 segment holding an Extended XMP fragment of one byte.
    fn fragment(i: u32) -> Vec<u8> {
        let mut body = EXTENDED_XMP_PREFIX.to_vec();
        body.extend_from_slice(&[b'0'; 32]);
        body.extend_from_slice(&1u32.to_be_bytes());
        body.extend_from_slice(&i.to_be_bytes());
        body.push(b'x');
        let length = u16::try_from(body.len() + 2).unwrap_or(u16::MAX);
        let mut seg = vec![0xFF, 0xE1];
        seg.extend_from_slice(&length.to_be_bytes());
        seg.extend_from_slice(&body);
        seg
    }

    /// Start of image, `n` fragments, then `tail`.
    fn jpeg(n: u32, tail: &[u8]) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8];
        for i in 0..n {
            out.extend_from_slice(&fragment(i));
        }
        out.extend_from_slice(tail);
        out
    }

    /// A start of scan, then bytes that look like another fragment inside the scan data.
    fn scan_then_fragment() -> Vec<u8> {
        let mut tail = vec![0xFF, 0xDA, 0x00, 0x02];
        tail.extend_from_slice(&fragment(99));
        tail.extend_from_slice(&[0xFF, 0xD9]);
        tail
    }

    #[test]
    fn counts_each_fragment_up_to_the_first_scan() {
        let data = jpeg(3, &scan_then_fragment());
        assert_eq!(extended_xmp_fragments(&data), 3);
    }

    #[test]
    fn other_app1_segments_and_fill_bytes_are_not_fragments() {
        let mut data = vec![0xFF, 0xD8, 0xFF, 0xFF];
        data.extend_from_slice(&[0xE1, 0x00, 0x08]);
        data.extend_from_slice(b"Exif\0\0");
        data.extend_from_slice(&fragment(0));
        data.extend_from_slice(&[0xFF, 0xD9]);
        assert_eq!(extended_xmp_fragments(&data), 1);
    }

    #[test]
    fn the_cap_is_inclusive() {
        let cap = u32::try_from(MAX_XMP_FRAGMENTS).unwrap_or(u32::MAX);
        assert!(!too_many_xmp_fragments(&jpeg(cap, &[0xFF, 0xD9])));
        assert!(too_many_xmp_fragments(&jpeg(cap + 1, &[0xFF, 0xD9])));
    }

    #[test]
    fn cut_off_and_malformed_headers_end_the_count_without_a_panic() {
        let whole = jpeg(2, &[0xFF, 0xD9]);
        for cut in 0..whole.len() {
            assert!(extended_xmp_fragments(&whole[..cut]) <= 2);
        }
        // A length below two still moves forward.
        assert_eq!(extended_xmp_fragments(&[0xFF, 0xD8, 0xFF, 0xE1, 0, 0]), 0);
        assert_eq!(extended_xmp_fragments(&[0xFF, 0xD8, 0x12]), 0);
        assert_eq!(extended_xmp_fragments(&[]), 0);
    }
}
