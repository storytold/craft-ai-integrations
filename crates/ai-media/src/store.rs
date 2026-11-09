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
//! - A busy host is tried again, a few times, with a doubling pause. Only failures that can pass
//!   are retried (no connection, a cut transfer, 408, 429, 500, 502, 503, 504); an expired or
//!   refused link, a page that is not media, a file over the limit and a refused address are
//!   answered at once. A pause that would take the time spent waiting past the retry budget is
//!   not taken: the store gives up and says so, rather than holding the job's worker for an hour
//!   because a host said `Retry-After: 3600`. Only pauses count against the budget; each
//!   transfer has its own time limit, so a long download that is cut can still carry on.
//! - A cut transfer whose host named the file's version carries on where it stopped on the next
//!   try, instead of starting over (see `partial.rs`). Without a version it starts over.
//! - A link that is clearly past its expiry (the earlier of the handle's `expires_at` and the
//!   expiry signed into the link) is refused before any request, and is not retried: it will not
//!   get better. Each redirect target is checked the same way before it is asked for.

use crate::expiry;
use crate::partial::{self, Partial};
use crate::sniff::{self, HEAD_LEN};
use crate::url_rules;
use craft_ai_core::retry::tried;
use craft_ai_core::{AiError, Backoff, ErrorKind, MediaType, Payload, PayloadHandle};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;
use url::Url;

/// How and where media is saved.
#[derive(Clone, Debug)]
pub struct MediaStoreConfig {
    /// An existing, absolute directory the app owns. The app decides when files are deleted.
    pub dir: PathBuf,
    /// Largest file accepted. Default 1 GiB.
    pub max_bytes: u64,
    pub connect_timeout: Duration,
    /// Time allowed for the whole transfer, including redirects. Default 30 minutes.
    pub transfer_timeout: Duration,
    pub max_redirects: u8,
    /// Permit links to a loopback host (http or https). For a connection the user runs on this
    /// computer, and for tests. Other private addresses stay refused.
    pub allow_loopback: bool,
    /// Most tries for one link, the first included. Clamped to 1..=5; 1 turns retries off.
    pub max_attempts: u8,
    /// Pause before the second try; each later pause doubles. Default 1 second.
    pub retry_delay: Duration,
    /// The most time the store will spend waiting between tries, in total (default 30 seconds).
    pub retry_budget: Duration,
}

impl MediaStoreConfig {
    /// The defaults for saving into `dir`: 1 GiB, 15 s to connect, 30 minutes per transfer, three
    /// redirects, no loopback, and three tries within 30 seconds.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            max_bytes: 1024 * 1024 * 1024,
            connect_timeout: Duration::from_secs(15),
            transfer_timeout: Duration::from_secs(30 * 60),
            max_redirects: 3,
            allow_loopback: false,
            max_attempts: 3,
            retry_delay: Duration::from_secs(1),
            retry_budget: Duration::from_secs(30),
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
        // A cut download carried from one try to the next; dropped (and deleted) on return.
        let mut kept: Option<Partial> = None;
        self.retrying(AiError::is_retryable, || self.fetch_once(handle, &mut kept))
    }

    /// Runs `call` under the store's retry rule. Unlike `Backoff::run`, only the pauses count
    /// against the budget, so a long transfer that is cut can still carry on.
    fn retrying<T>(
        &self,
        worth_retrying: fn(&AiError) -> bool,
        mut call: impl FnMut() -> Result<T, AiError>,
    ) -> Result<T, AiError> {
        let backoff = Backoff {
            max_attempts: self.config.max_attempts,
            first_delay: self.config.retry_delay,
            budget: self.config.retry_budget,
        };
        let mut paused = Duration::ZERO;
        let mut attempt = 1u8;
        loop {
            let error = match call() {
                Ok(value) => return Ok(value),
                Err(error) => error,
            };
            let wait = if worth_retrying(&error) {
                backoff.next_wait(attempt, &error, paused)
            } else {
                None
            };
            let Some(wait) = wait else {
                return Err(tried(error, attempt));
            };
            std::thread::sleep(wait);
            paused = paused.saturating_add(wait);
            attempt = attempt.saturating_add(1);
        }
    }

    /// One try. With `kept` set it asks only for the missing bytes; whatever the answer, `kept`
    /// ends up holding the download to resume next time, or nothing.
    fn fetch_once(
        &self,
        handle: &PayloadHandle,
        kept: &mut Option<Partial>,
    ) -> Result<PayloadHandle, AiError> {
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
                ));
            }
            url_rules::check_resolved(&url, self.config.allow_loopback)?;
            // The agent itself carries no proxy, so a loopback link can never leave this
            // machine; a hosted link may use the environment's proxy, hop by hop.
            let mut request = if url_rules::is_loopback(&url) {
                self.agent.get(url.as_str())
            } else {
                self.agent
                    .get(url.as_str())
                    .config()
                    .proxy(craft_ai_http::hosted_proxy())
                    .build()
            };
            if let Some(part) = kept.as_ref() {
                request = request
                    .header("range", format!("bytes={}-", part.written))
                    .header("if-range", part.validator.as_str());
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
                    ));
                }
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                let next = url_rules::redirect_target(&url, location)?;
                url = next;
                continue;
            }
            if status == 429 {
                let error =
                    AiError::new(ErrorKind::RateLimited, "the media host asked to slow down");
                return Err(match retry_after(&response) {
                    Some(secs) => error.with_retry_after(secs),
                    None => error,
                });
            }
            if matches!(status, 408 | 500 | 502 | 503 | 504) {
                return Err(AiError::new(
                    ErrorKind::Network,
                    format!("the media host answered {status}"),
                ));
            }
            if status == 416 && kept.take().is_some() {
                return Err(AiError::new(
                    ErrorKind::Network,
                    "the media host could not send the rest; starting over",
                ));
            }
            if !(200..300).contains(&status) {
                return Err(AiError::new(
                    ErrorKind::Provider,
                    format!("media link answered {status}; it may have expired"),
                ));
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
            return Err(too_large());
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
    ) -> Result<PayloadHandle, AiError> {
        let Some(mut part) = kept.take() else {
            return Err(AiError::new(
                ErrorKind::Network,
                "the media host sent part of a file that was not asked for",
            ));
        };
        if part.source != source.as_str() {
            return Err(AiError::new(
                ErrorKind::Network,
                "the media link now leads to another address; starting over",
            ));
        }
        if partial::contradicts(&part.validator, response.headers()) {
            return Err(AiError::new(
                ErrorKind::Network,
                "the media host sent a piece of another version; starting over",
            ));
        }
        match partial::content_range(response.headers()) {
            Some((start, end, total)) if start == part.written => {
                if total.is_some_and(|t| t > self.config.max_bytes) {
                    return Err(too_large());
                }
                if total.zip(part.whole).is_some_and(|(t, w)| t != w) {
                    return Err(AiError::new(
                        ErrorKind::Network,
                        "the media host gave the file another size; starting over",
                    ));
                }
                part.whole = total.or(part.whole);
                let body = response.into_body().into_reader();
                self.fill(part, body, Some(end.saturating_add(1)), kept)
            }
            _ => Err(AiError::new(
                ErrorKind::Network,
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
    ) -> Result<PayloadHandle, AiError> {
        let mut head = [0u8; HEAD_LEN];
        let mut filled = 0;
        while filled < HEAD_LEN {
            match body.read(&mut head[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(_) => return Err(transfer_failed()),
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
            ));
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
            validator: version.unwrap_or_default(),
            media,
            source: from.0.to_string(),
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
    ) -> Result<PayloadHandle, AiError> {
        let room = self.config.max_bytes.saturating_sub(part.written);
        match std::io::copy(&mut body.take(room + 1), part.temp.as_file_mut()) {
            Ok(copied) => part.written += copied,
            Err(_) => {
                // A failed read writes nothing, so the file's length is what arrived intact.
                let on_disk = part.temp.as_file().metadata().map(|m| m.len());
                if let Ok(len) = on_disk
                    && !part.validator.is_empty()
                    && len <= self.config.max_bytes
                    && end.is_none_or(|e| len <= e)
                {
                    part.written = len;
                    *kept = Some(part);
                }
                return Err(transfer_failed());
            }
        }
        if part.written > self.config.max_bytes {
            return Err(too_large());
        }
        if end.is_some_and(|e| part.written > e) {
            return Err(AiError::new(
                ErrorKind::Network,
                "the media host sent more than it said; starting over",
            ));
        }
        // Short of what the headers promised: cut, however cleanly the connection closed.
        let short =
            end.is_some_and(|e| part.written < e) || part.whole.is_some_and(|w| part.written < w);
        if short {
            if !part.validator.is_empty() {
                *kept = Some(part);
            }
            return Err(transfer_failed());
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
            _ => Err(AiError::new(
                ErrorKind::Internal,
                "could not describe the media file",
            )),
        }
    }
}

/// Seconds from a `Retry-After` header. A date form is ignored, so the normal pause applies.
fn retry_after(response: &ureq::http::Response<ureq::Body>) -> Option<u64> {
    response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
}

/// A transfer that stopped part way. Network, so the store tries again.
fn transfer_failed() -> AiError {
    AiError::new(ErrorKind::Network, "media transfer failed")
}

/// The disk would not take the file.
fn disk(_: std::io::Error) -> AiError {
    AiError::new(ErrorKind::Internal, "could not write the media file")
}

/// The error for media past the configured size limit.
fn too_large() -> AiError {
    AiError::new(
        ErrorKind::Provider,
        "the media is larger than the configured limit",
    )
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
