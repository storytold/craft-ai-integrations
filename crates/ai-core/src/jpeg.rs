//! A check every JPEG decoder in the workspace runs before it hands a file to `image`/`zune-jpeg`.
//!
//! `zune-jpeg` re-sorts and re-scans every Extended XMP fragment collected so far after each
//! header segment, so a file made of thousands of tiny fragments costs it quadratic time before
//! any size limit applies. A real file needs a handful; [`MAX_XMP_FRAGMENTS`] fragments hold
//! 64 MB of XMP. Invariant: a decoder that calls [`too_many_xmp_fragments`] first and refuses on
//! `true` never passes the decoder more than that many fragments.
//!
//! The count does not walk the header segments. The decoder runs in a lenient mode that skips
//! stray bytes between segments and keeps reading, so a walk that stops where the file stops
//! looking well formed would under-count. Instead every occurrence of the Extended XMP namespace
//! anywhere in the file is counted. The decoder only keeps a fragment whose APP1 body starts with
//! that namespace, so the count is never below what it can collect, whatever comes before. A
//! real file never repeats the namespace a thousand times by chance. The count is one pass over
//! the file, and it reads only bytes, with no I/O, so it compiles for wasm with this crate.

/// The most Extended XMP fragments (APP1 segments) a JPEG may carry before it is refused.
pub const MAX_XMP_FRAGMENTS: usize = 1024;

/// The namespace that opens every Extended XMP fragment's APP1 body.
pub const EXTENDED_XMP_PREFIX: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";

/// An upper bound on the Extended XMP fragments a decoder can collect from `data`: the number of
/// times the Extended XMP namespace occurs anywhere in it.
pub fn extended_xmp_fragments(data: &[u8]) -> usize {
    data.windows(EXTENDED_XMP_PREFIX.len())
        .filter(|w| *w == EXTENDED_XMP_PREFIX)
        .count()
}

/// Whether a JPEG may carry more than [`MAX_XMP_FRAGMENTS`] Extended XMP fragments, and so must
/// be refused before it reaches a decoder.
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

    #[test]
    fn counts_each_fragment() {
        assert_eq!(extended_xmp_fragments(&jpeg(3, &[0xFF, 0xD9])), 3);
    }

    /// The decoder skips stray bytes between segments and keeps reading, so a fragment after one
    /// still counts. So does one after an unknown marker or past a scan: an over-count is safe.
    #[test]
    fn fragments_after_stray_bytes_or_odd_markers_still_count() {
        let mut data = vec![0xFF, 0xD8];
        data.extend_from_slice(&fragment(0));
        data.push(0x42);
        data.extend_from_slice(&fragment(1));
        data.extend_from_slice(&[0xFF, 0x02, 0xFF, 0xFF]);
        data.extend_from_slice(&fragment(2));
        data.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02]);
        data.extend_from_slice(&fragment(3));
        assert_eq!(extended_xmp_fragments(&data), 4);
    }

    #[test]
    fn other_app1_segments_are_not_fragments() {
        let mut data = vec![0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x08];
        data.extend_from_slice(b"Exif\0\0");
        data.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x20]);
        data.extend_from_slice(b"http://ns.adobe.com/xap/1.0/\0");
        data.extend_from_slice(&fragment(0));
        assert_eq!(extended_xmp_fragments(&data), 1);
    }

    #[test]
    fn the_cap_is_inclusive() {
        let cap = u32::try_from(MAX_XMP_FRAGMENTS).unwrap_or(u32::MAX);
        assert!(!too_many_xmp_fragments(&jpeg(cap, &[0xFF, 0xD9])));
        assert!(too_many_xmp_fragments(&jpeg(cap + 1, &[0xFF, 0xD9])));
    }

    #[test]
    fn short_and_empty_input_count_nothing() {
        assert_eq!(extended_xmp_fragments(&[]), 0);
        assert_eq!(extended_xmp_fragments(&EXTENDED_XMP_PREFIX[1..]), 0);
        assert_eq!(extended_xmp_fragments(EXTENDED_XMP_PREFIX), 1);
    }
}
