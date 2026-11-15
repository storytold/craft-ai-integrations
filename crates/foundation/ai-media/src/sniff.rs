//! Identify media by its first bytes, never by a label a server chose.
//!
//! Only formats the Craft apps can use are recognised. Anything else (an HTML error page, a JSON
//! body, a truncated file) returns `None`, which the caller treats as "not the media we asked for".

/// Bytes the caller must have read before calling [`media_type`].
pub const HEAD_LEN: usize = 16;

/// The media type and a file extension for the bytes, or `None` when unrecognised.
pub fn media_type(head: &[u8]) -> Option<(&'static str, &'static str)> {
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(("image/png", "png"));
    }
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(("image/jpeg", "jpg"));
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return Some(("image/gif", "gif"));
    }
    if head.len() >= 12 && &head[..4] == b"RIFF" {
        return match &head[8..12] {
            b"WEBP" => Some(("image/webp", "webp")),
            b"WAVE" => Some(("audio/wav", "wav")),
            _ => None,
        };
    }
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        return match &head[8..12] {
            b"avif" | b"avis" => Some(("image/avif", "avif")),
            b"heic" | b"heix" | b"mif1" => Some(("image/heic", "heic")),
            [b'M', b'4', b'A', _] => Some(("audio/mp4", "m4a")),
            _ => Some(("video/mp4", "mp4")),
        };
    }
    if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some(("video/webm", "webm"));
    }
    if head.starts_with(b"ID3") || matches!(head, [0xFF, 0xFB | 0xF3 | 0xF2, ..]) {
        return Some(("audio/mpeg", "mp3"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn padded(prefix: &[u8]) -> Vec<u8> {
        let mut v = prefix.to_vec();
        v.resize(HEAD_LEN, 0);
        v
    }

    #[test]
    fn recognises_each_supported_format() {
        let cases: [(&[u8], &str); 8] = [
            (b"\x89PNG\r\n\x1a\n", "image/png"),
            (&[0xFF, 0xD8, 0xFF, 0xE0], "image/jpeg"),
            (b"GIF89a", "image/gif"),
            (b"RIFF\0\0\0\0WEBP", "image/webp"),
            (b"RIFF\0\0\0\0WAVE", "audio/wav"),
            (b"\0\0\0\x18ftypisom", "video/mp4"),
            (&[0x1A, 0x45, 0xDF, 0xA3], "video/webm"),
            (b"ID3\x04", "audio/mpeg"),
        ];
        for (prefix, expected) in cases {
            assert_eq!(media_type(&padded(prefix)).map(|m| m.0), Some(expected));
        }
    }

    #[test]
    fn brands_in_an_iso_container_pick_the_right_kind() {
        assert_eq!(
            media_type(&padded(b"\0\0\0\x18ftypM4A ")).map(|m| m.0),
            Some("audio/mp4")
        );
        assert_eq!(
            media_type(&padded(b"\0\0\0\x18ftypavif")).map(|m| m.0),
            Some("image/avif")
        );
    }

    #[test]
    fn text_pages_and_short_input_are_not_media() {
        assert_eq!(media_type(&padded(b"<!DOCTYPE html>")), None);
        assert_eq!(media_type(&padded(b"{\"error\":\"expired\"}")), None);
        assert_eq!(media_type(&padded(b"RIFF\0\0\0\0AVI ")), None);
        assert_eq!(media_type(b""), None);
        assert_eq!(media_type(b"\x89PN"), None);
    }
}
