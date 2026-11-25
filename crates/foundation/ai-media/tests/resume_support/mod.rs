//! Shared helpers for the resume tests: a picture to cut, a scripted localhost server and a
//! store that saves from it.

use crate::support::{Recorded, Reply, Server, http_bytes};
use craft_ai_core::{AiError, Payload, PayloadHandle};
use craft_ai_media::{MediaStore, MediaStoreConfig};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A picture long enough to cut part way: a PNG signature, then filler.
pub fn picture() -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    bytes.extend((0..200u8).map(|i| b'a' + i % 26));
    bytes
}

/// The same size, but a different file from its first bytes on: another resource entirely.
pub fn other_picture() -> Vec<u8> {
    let mut bytes = picture();
    bytes[20] = b'Q';
    bytes
}

/// The same size, but a different file: the host's newer version.
pub fn newer_picture() -> Vec<u8> {
    let mut bytes = picture();
    let last = bytes.len() - 1;
    bytes[last] = b'Z';
    bytes
}

pub const CUT: usize = 40;

/// A store config in `dir` that may reach localhost and does not wait between tries.
pub fn config(dir: &Path) -> MediaStoreConfig {
    let mut cfg = MediaStoreConfig::new(dir);
    cfg.allow_loopback = true;
    cfg.retry.first_delay = Duration::ZERO;
    cfg
}

/// Saves the server's `/media/out.png` through a fresh store in `dir`.
pub fn save(server: &Server, dir: &Path) -> Result<PayloadHandle, AiError> {
    save_with(server, config(dir))
}

/// Saves the server's `/media/out.png` through a fresh store with `cfg`.
pub fn save_with(server: &Server, cfg: MediaStoreConfig) -> Result<PayloadHandle, AiError> {
    let uri = format!("{}/media/out.png", server.url.trim_end_matches("/v1"));
    let Payload::Handle(handle) = Payload::handle(&uri, "image/png", None).unwrap() else {
        unreachable!()
    };
    MediaStore::new(cfg).unwrap().save(&handle)
}

/// A redirect to `path` on the same server.
pub fn moved(path: &'static str) -> Answer {
    Box::new(move |_| http_bytes(302, &[("Location", path)], b""))
}

/// The bytes of the local file a saved handle points at.
pub fn saved_bytes(handle: &PayloadHandle) -> Vec<u8> {
    let path: PathBuf = url::Url::parse(&handle.uri)
        .unwrap()
        .to_file_path()
        .unwrap();
    std::fs::read(path).unwrap()
}

/// A 200 that promises the whole picture, sends the first `CUT` bytes and closes.
pub fn cut_whole(extra_headers: &str) -> Reply {
    let full = picture();
    let mut raw = format!(
        "HTTP/1.1 200 X\r\nContent-Length: {}\r\n{extra_headers}Connection: close\r\n\r\n",
        full.len()
    )
    .into_bytes();
    raw.extend_from_slice(&full[..CUT]);
    Reply::Raw(raw)
}

/// A 206 carrying `bytes[start..]` of `full`, labelled as starting at `label_start`.
pub fn piece(full: &[u8], start: usize, label_start: usize) -> Reply {
    let range = format!(
        "bytes {label_start}-{}/{}",
        label_start + full.len() - start - 1,
        full.len()
    );
    http_bytes(206, &[("Content-Range", range.as_str())], &full[start..])
}

/// One scripted answer: the reply to send for a recorded request.
pub type Answer = Box<dyn Fn(&Recorded) -> Reply + Send + Sync>;

/// A server that answers each request with the next scripted reply, recording what it was sent.
pub fn scripted(replies: Vec<Answer>) -> Server {
    let next = Arc::new(AtomicUsize::new(0));
    Server::start(move |request| {
        let i = next.fetch_add(1, Ordering::SeqCst);
        replies[i.min(replies.len() - 1)](request)
    })
}

/// The `name` header of the server's `i`th request, if it was sent.
pub fn sent(server: &Server, i: usize, name: &str) -> Option<String> {
    server.requests.lock().unwrap()[i]
        .header(name)
        .map(str::to_owned)
}
