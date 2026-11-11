//! Copies one hosted media link into an app-owned directory.
//!
//! Invariants:
//! - No credential of any kind is sent. The request carries no authorization header and the agent
//!   keeps no cookies, so a delivery host or a redirect target never sees a provider key.
//! - Redirects are followed by hand, a few at most, and every hop passes the same URL rules as the
//!   first link. A hop may not downgrade from https.
//! - The body is bounded, identified by its first bytes (not by `Content-Type`), and written to a
//!   temporary file in the directory. Anything that fails leaves no file behind.
//! - The saved file's media type comes from its bytes. It must be the same kind (image, video,
//!   audio) the provider declared.
//! - A busy host is tried again, a few times, with a doubling pause (`Backoff::run`). Only
//!   failures that can pass are retried (no connection, a cut transfer, 408, 429, 500, 502, 503,
//!   504); an expired or refused link, a page that is not media, a file over the limit and a
//!   refused address are answered at once. Only pauses count against the retry budget, so a
//!   long download that is cut can still carry on.
//! - The whole save, every try, redirect and pause included, ends within `transfer_timeout`:
//!   each request gets only the time left. A host that keeps cutting the transfer cannot hold
//!   the job's worker for the number of tries times that limit.
//! - A cut transfer whose host named the file's version carries on where it stopped on the next
//!   try, instead of starting over (see `partial.rs`). Without a version it starts over. When
//!   the host's answer does not continue the kept copy (a 416, another version, a piece that
//!   does not fit), the copy is dropped and the whole file is asked for at once, in the same
//!   try: a host that has moved on is not a failure, and costs no pause or try.
//! - A link that is clearly past its expiry (the earlier of the handle's `expires_at` and the
//!   expiry signed into the link) is refused before any request, and is not retried: it will not
//!   get better. Each redirect target is checked the same way before it is asked for.

use crate::expiry;
use crate::partial::{self, Partial};
use crate::sniff::{self, HEAD_LEN};
use crate::url_rules;
use craft_ai_core::{AiError, Backoff, ErrorKind, MediaType, Payload, PayloadHandle};
use errors::{Stop, disk, out_of_time, retry_after, too_large, transfer_failed};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use url::Url;

#[path = "store_errors.rs"]
mod errors;

/// How and where media is saved.
#[derive(Clone, Debug)]
pub struct MediaStoreConfig {
    /// An existing, absolute directory the app owns. The app decides when files are deleted.
    pub dir: PathBuf,
    /// Largest file accepted. Default 1 GiB.
    pub max_bytes: u64,
    pub connect_timeout: Duration,
    /// Time allowed for the whole save: every try, redirect and pause between tries. Default 30
    /// minutes.
    pub transfer_timeout: Duration,
    pub max_redirects: u8,
    /// Permit links to a loopback host (http or https). For a connection the user runs on this
    /// computer, and for tests. Other private addresses stay refused.
    pub allow_loopback: bool,
    /// How a busy host is tried again (default [`Backoff::STANDARD`]: three tries, a one-second
    /// first pause, 30 seconds of pauses at most). [`Backoff::NEVER`] turns retries off.
    pub retry: Backoff,
}

impl MediaStoreConfig {
    /// The defaults for saving into `dir`: 1 GiB, 15 s to connect, 30 minutes per save, three
    /// redirects, no loopback, and three tries within 30 seconds of pauses.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            max_bytes: 1024 * 1024 * 1024,
            connect_timeout: Duration::from_secs(15),
            transfer_timeout: Duration::from_secs(30 * 60),
            max_redirects: 3,
            allow_loopback: false,
            retry: Backoff::STANDARD,
        }
    }
}

/// Saves hosted media to disk.
pub struct MediaStore {
    config: MediaStoreConfig,
    agent: ureq::Agent,
}

impl std::fmt::Debug for MediaStore {
    /// Shows the directory only.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaStore")
            .field("dir", &self.config.dir)
            .finish_non_exhaustive()
    }
}

impl MediaStore {
    /// Checks the directory and size limit and builds the downloader. Nothing is fetched yet.
    pub fn new(config: MediaStoreConfig) -> Result<Self, AiError> {
        if !config.dir.is_absolute() || !config.dir.is_dir() {
            return Err(AiError::invalid(
                "the media directory must be an existing absolute folder",
            ));
        }
        if config.max_bytes == 0 {
            return Err(AiError::invalid("the media size limit must be above zero"));
        }
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .tls_config(craft_ai_http::tls_config())
                .proxy(craft_ai_http::loopback_proxy())
                .http_status_as_error(false)
                .max_redirects(0)
                .max_redirects_will_error(false)
                .timeout_connect(Some(config.connect_timeout))
                .timeout_global(Some(config.transfer_timeout))
                .build(),
        );
        Ok(Self { config, agent })
    }

    /// True when the handle names a link we would download (not a `file:` or `data:` handle).
    pub fn is_remote(handle: &PayloadHandle) -> bool {
        let uri = handle.uri.as_bytes();
        let starts = |p: &[u8]| uri.len() >= p.len() && uri[..p.len()].eq_ignore_ascii_case(p);
        starts(b"http://") || starts(b"https://")
    }

    /// Downloads the link and returns a `file:` handle to the saved copy, trying again when the
    /// host is busy (see the module notes). Blocks for the pauses, so call it from a worker.
    pub fn save(&self, handle: &PayloadHandle) -> Result<PayloadHandle, AiError> {
        // None only when the limit is too far off for the clock: then each request's own limit.
        let deadline = Instant::now().checked_add(self.config.transfer_timeout);
        let in_time = || deadline.is_none_or(|d| Instant::now() < d);
        // A cut download carried from one try to the next; dropped (and deleted) on return.
        let mut kept: Option<Partial> = None;
        self.config.retry.run(
            |error: &AiError| error.is_retryable() && in_time(),
            || {
                self.fetch_once(handle, &mut kept, deadline)
                    .map_err(|e| if in_time() { e } else { out_of_time() })
            },
            std::thread::sleep,
        )
    }

    /// One try. With `kept` set it asks only for the missing bytes; whatever the answer, `kept`
    /// ends up holding the download to resume next time, or nothing. When the answer does not
    /// continue the kept copy, the whole file is asked for at once.
    fn fetch_once(
        &self,
        handle: &PayloadHandle,
        kept: &mut Option<Partial>,
        deadline: Option<Instant>,
    ) -> Result<PayloadHandle, AiError> {
        let resuming = kept.is_some();
        match self.ask(handle, kept, deadline) {
            // The copy is gone now, so this asks without a range and cannot start over again.
            Err(Stop::StartOver(_)) if resuming => {
                self.ask(handle, kept, deadline).map_err(Stop::into_error)
            }
            answer => answer.map_err(Stop::into_error),
        }
    }

    /// One request for the link, following redirects, and its body saved or kept.
    fn ask(
        &self,
        handle: &PayloadHandle,
        kept: &mut Option<Partial>,
        deadline: Option<Instant>,
    ) -> Result<PayloadHandle, Stop> {
        let mut url = Url::parse(&handle.uri)
            .map_err(|_| AiError::new(ErrorKind::Provider, "media link is not a valid URL"))?;
        let mut hops = 0u8;
        let response = loop {
            // A redirect may lead to a signed link of its own, so every hop is checked.
            if let Some(at) = expiry::earliest(handle.expires_at, &url)
                && expiry::clearly_expired(at, expiry::now_unix())
            {
                return Err(AiError::new(
                    ErrorKind::Provider,
                    format!("the media link expired at {}", expiry::utc_text(at)),
                )
                .into());
            }
            url_rules::check_resolved(&url, self.config.allow_loopback)?;
            let left = match deadline {
                Some(d) => d.saturating_duration_since(Instant::now()),
                None => self.config.transfer_timeout,
            };
            if left.is_zero() {
                return Err(out_of_time().into());
            }
            let request = self
                .agent
                .get(url.as_str())
                .config()
                .timeout_global(Some(left));
            // The agent itself carries no proxy, so a loopback link can never leave this
            // machine; a hosted link may use the environment's proxy, hop by hop.
            let mut request = if url_rules::is_loopback(&url) {
                request.build()
            } else {
                request.proxy(craft_ai_http::hosted_proxy()).build()
            };
            if let Some(part) = kept.as_ref()
                && let Some(version) = part.validator.as_deref()
            {
                request = request
                    .header("range", format!("bytes={}-", part.written))
                    .header("if-range", version);
            }
            let response = request
                .call()
                .map_err(|_| AiError::new(ErrorKind::Network, "could not download the media"))?;
            let status = response.status().as_u16();
            if (300..400).contains(&status) {
                hops += 1;
                if hops > self.config.max_redirects {
                    return Err(AiError::new(
                        ErrorKind::Provider,
                        "media link redirects too often",
                    )
                    .into());
                }
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                url = url_rules::redirect_target(&url, location)?;
                continue;
            }
            if status == 429 {
                let error =
                    AiError::new(ErrorKind::RateLimited, "the media host asked to slow down");
                return Err(match retry_after(&response) {
                    Some(secs) => error.with_retry_after(secs),
                    None => error,
                }
                .into());
            }
            if matches!(status, 408 | 500 | 502 | 503 | 504) {
                let busy = format!("the media host answered {status}");
                return Err(AiError::new(ErrorKind::Network, busy).into());
            }
            if status == 416 && kept.take().is_some() {
                return Err(Stop::StartOver(
                    "the media host could not send the rest; starting over",
                ));
            }
            if !(200..300).contains(&status) {
                let refused = format!("media link answered {status}; it may have expired");
                return Err(AiError::new(ErrorKind::Provider, refused).into());
            }
            break response;
        };
        if response.status().as_u16() == 206 {
            return self.resume(kept, &url, response);
        }
        // A whole file: whatever was kept is out of date, or the host ignored the range.
        *kept = None;
        let declared = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        if declared.is_some_and(|d| d > self.config.max_bytes) {
            return Err(too_large().into());
        }
        let version = partial::validator(response.headers());
        self.write(
            response.into_body().into_reader(),
            &handle.media_type,
            version,
            (&url, declared),
            kept,
        )
    }

    /// Takes a piece (206) onto the kept copy, if it came from the copy's own address (`source`)
    /// and starts exactly where the copy ends.
    fn resume(
        &self,
        kept: &mut Option<Partial>,
        source: &Url,
        response: ureq::http::Response<ureq::Body>,
    ) -> Result<PayloadHandle, Stop> {
        let Some(mut part) = kept.take() else {
            return Err(AiError::new(
                ErrorKind::Network,
                "the media host sent part of a file that was not asked for",
            )
            .into());
        };
        if part.source != partial::address(source) {
            return Err(Stop::StartOver(
                "the media link now leads to another address; starting over",
            ));
        }
        let headers = response.headers();
        if (part.validator.as_deref()).is_none_or(|v| partial::contradicts(v, headers)) {
            return Err(Stop::StartOver(
                "the media host sent a piece of another version; starting over",
            ));
        }
        match partial::content_range(headers) {
            Some((start, end, total)) if start == part.written => {
                if total.is_some_and(|t| t > self.config.max_bytes) {
                    return Err(too_large().into());
                }
                if total.zip(part.whole).is_some_and(|(t, w)| t != w) {
                    return Err(Stop::StartOver(
                        "the media host gave the file another size; starting over",
                    ));
                }
                part.whole = total.or(part.whole);
                let body = response.into_body().into_reader();
                self.fill(part, body, Some(end.saturating_add(1)), kept)
            }
            _ => Err(Stop::StartOver(
                "the media host sent a piece that does not continue the download; starting over",
            )),
        }
    }

    /// Reads the first bytes to learn the media type, then saves the whole body. `from` is the
    /// address the body came from and the length its headers declared. A cut transfer with a
    /// `version` leaves the download in `kept` for the next try.
    fn write(
        &self,
        mut body: impl Read,
        declared: &MediaType,
        version: Option<String>,
        from: (&Url, Option<u64>),
        kept: &mut Option<Partial>,
    ) -> Result<PayloadHandle, Stop> {
        let mut head = [0u8; HEAD_LEN];
        let mut filled = 0;
        while filled < HEAD_LEN {
            match body.read(&mut head[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(_) => return Err(transfer_failed().into()),
            }
        }
        let (media, extension) = sniff::media_type(&head[..filled])
            .ok_or_else(|| AiError::new(ErrorKind::Provider, "the link did not return media"))?;
        let sniffed = MediaType::new(media)
            .map_err(|_| AiError::new(ErrorKind::Internal, "could not name the media type"))?;
        if sniffed.kind() != declared.kind() {
            return Err(AiError::new(
                ErrorKind::Provider,
                "the link returned a different kind of media than the provider declared",
            )
            .into());
        }
        let mut temp = tempfile::Builder::new()
            .prefix("craft-ai-")
            .suffix(&format!(".{extension}"))
            .tempfile_in(&self.config.dir)
            .map_err(|_| AiError::new(ErrorKind::Internal, "could not create the media file"))?;
        temp.as_file_mut()
            .write_all(&head[..filled])
            .map_err(disk)?;
        let part = Partial {
            temp,
            written: filled as u64,
            validator: version,
            media,
            source: partial::address(from.0),
            whole: from.1,
        };
        self.fill(part, body, from.1, kept)
    }

    /// Appends the body to the copy and, once it is all there, keeps the file. `end` is the length
    /// the copy should reach with this body, when the headers said; the copy's `whole` is the
    /// file's. A cut transfer, or a piece that stops before the file does, leaves the copy in
    /// `kept` when it has a version to resume against and nothing past `end`; otherwise it is
    /// dropped, which deletes it.
    fn fill(
        &self,
        mut part: Partial,
        body: impl Read,
        end: Option<u64>,
        kept: &mut Option<Partial>,
    ) -> Result<PayloadHandle, Stop> {
        // Only a copy with a version is kept; one without is dropped, which deletes it.
        let keep = |part: Partial, kept: &mut Option<Partial>| {
            if part.validator.is_some() {
                *kept = Some(part);
            }
        };
        let room = self.config.max_bytes.saturating_sub(part.written);
        match copy_body(body.take(room + 1), part.temp.as_file_mut()) {
            Ok(copied) => part.written += copied,
            // The disk would not take the bytes: asking the host again will not help, and the
            // copy is dropped.
            Err(CopyFailed::Disk(error)) => return Err(disk(error).into()),
            Err(CopyFailed::Cut(copied)) => {
                // Every byte read before the cut was written, so the copy is intact up to here.
                let len = part.written.saturating_add(copied);
                let fits = len <= self.config.max_bytes;
                if fits && end.is_none_or(|e| len <= e) {
                    part.written = len;
                    keep(part, kept);
                }
                return Err(transfer_failed().into());
            }
        }
        if part.written > self.config.max_bytes {
            return Err(too_large().into());
        }
        if end.is_some_and(|e| part.written > e) {
            return Err(Stop::StartOver(
                "the media host sent more than it said; starting over",
            ));
        }
        // Short of what the headers promised: cut, however cleanly the connection closed.
        let short =
            end.is_some_and(|e| part.written < e) || part.whole.is_some_and(|w| part.written < w);
        if short {
            keep(part, kept);
            return Err(transfer_failed().into());
        }
        let Partial {
            temp,
            written: size,
            media,
            ..
        } = part;
        temp.as_file().sync_all().map_err(disk)?;
        let uri = Url::from_file_path(temp.path())
            .map_err(|()| AiError::new(ErrorKind::Internal, "could not name the media file"))?;
        temp.keep()
            .map_err(|_| AiError::new(ErrorKind::Internal, "could not keep the media file"))?;
        match Payload::handle(uri.as_str(), media, Some(size))? {
            Payload::Handle(saved) => Ok(saved),
            _ => Err(AiError::new(ErrorKind::Internal, "could not describe the media file").into()),
        }
    }
}

/// Why copying a body into the file stopped early.
enum CopyFailed {
    /// The body could not be read: a cut transfer, after this many bytes were copied.
    Cut(u64),
    /// The file would not take the bytes.
    Disk(std::io::Error),
}

/// Copies `body` into `file` and returns the bytes copied, telling a failed read (a cut
/// transfer) from a failed write (the disk), which `std::io::copy` does not.
fn copy_body(mut body: impl Read, file: &mut impl Write) -> Result<u64, CopyFailed> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut copied = 0u64;
    loop {
        let n = match body.read(&mut buf) {
            Ok(0) => return Ok(copied),
            Ok(n) => n.min(buf.len()),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(CopyFailed::Cut(copied)),
        };
        file.write_all(&buf[..n]).map_err(CopyFailed::Disk)?;
        copied = copied.saturating_add(n as u64);
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
