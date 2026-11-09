//! Tests for the store's own transfer limit, which need its private `write`.

use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A PNG header followed by endless zeros, counting what was pulled from it.
struct Endless {
    sent: u64,
    pulled: Arc<AtomicU64>,
    limit: u64,
}

impl Read for Endless {
    /// Fills `buf` with the header, then zeros, until `limit` bytes are sent, and counts them.
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let header = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        let mut n = 0;
        for slot in buf.iter_mut() {
            if self.sent >= self.limit {
                break;
            }
            *slot = header.get(self.sent as usize).copied().unwrap_or(0);
            self.sent += 1;
            n += 1;
        }
        self.pulled.fetch_add(n as u64, Ordering::SeqCst);
        Ok(n)
    }
}

/// An endless body is cut off near the size limit while it is read, not read to the end and
/// judged afterwards, and nothing is left in the folder.
#[test]
fn the_running_limit_stops_the_transfer_not_just_the_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = MediaStoreConfig::new(dir.path());
    config.max_bytes = 100;
    let store = MediaStore::new(config).unwrap();
    let pulled = Arc::new(AtomicU64::new(0));
    let body = Endless {
        sent: 0,
        pulled: Arc::clone(&pulled),
        limit: 5 * 1024 * 1024,
    };
    let declared = MediaType::new("image/png").unwrap();
    let from = Url::parse("https://media.example/out.png").unwrap();
    let error = store
        .write(body, &declared, None, (&from, None), &mut None)
        .unwrap_err();
    assert!(error.message.contains("larger than"));
    assert!(pulled.load(Ordering::SeqCst) <= 101 + 8192, "{pulled:?}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
