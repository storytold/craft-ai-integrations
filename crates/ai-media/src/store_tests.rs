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
        .map_err(errors::Stop::into_error)
        .unwrap_err();
    assert!(error.message.contains("larger than"));
    assert!(pulled.load(Ordering::SeqCst) <= 101 + 8192, "{pulled:?}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

/// A body that gives `bytes`, then fails as a cut connection does.
struct CutAfter(Vec<u8>);

impl Read for CutAfter {
    /// Hands out the bytes, then an error once they are gone.
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.0.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "cut",
            ));
        }
        let n = buf.len().min(self.0.len());
        buf[..n].copy_from_slice(&self.0[..n]);
        self.0.drain(..n);
        Ok(n)
    }
}

/// A file that will not take any bytes, as a full disk does.
struct FullDisk;

impl Write for FullDisk {
    /// Always fails.
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(std::io::ErrorKind::StorageFull, "full"))
    }

    /// Nothing to flush.
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A failed read is a cut transfer that says how much arrived; a failed write is the disk's
/// fault, not the host's, so it is not reported as a cut transfer to be tried again.
#[test]
fn a_failed_write_is_a_disk_error_not_a_cut() {
    let mut file = Vec::new();
    let read = copy_body(CutAfter(vec![7; 10]), &mut file);
    assert!(matches!(read, Err(CopyFailed::Cut(10))));
    assert_eq!(file, vec![7; 10]);
    let write = copy_body(CutAfter(vec![7; 10]), &mut FullDisk);
    assert!(matches!(write, Err(CopyFailed::Disk(_))));
    assert_eq!(disk(std::io::Error::other("x")).kind, ErrorKind::Internal);
}
