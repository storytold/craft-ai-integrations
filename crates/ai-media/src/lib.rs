//! Durable local copies of media that hosted providers return as short-lived links.
//!
//! Hosted image and video providers answer a finished job with a URL on a delivery host, and that
//! URL expires. [`PersistingProvider`] wraps any [`craft_ai_core::Provider`]: when a poll carries
//! an output that is an http(s) link, it downloads the file into an app-owned directory and
//! replaces the link with a `file:` handle. Everything else passes through untouched.
//!
//! If a copy cannot be made (bad link, expired, too large, not media, disk problem) the job is
//! not failed, because the provider already did the paid work. The output keeps its original
//! link and a [`Event::Notice`] says no local copy was saved. The notice never contains the link,
//! which can embed a signed token. When the link's expiry is known (signed into it, or already on
//! the handle) the kept handle carries it as `expires_at` and the notice says when the link stops
//! working, so the app can tell the person to save the result in time.
//!
//! The app owns the directory, its retention and its cleanup. Downloads are synchronous inside
//! `poll`, which the client already runs on a worker thread.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

/// Reading and writing the masks the inpainting adapters send.
/// When a signed media link stops working.
pub mod expiry;
mod image_size;
pub mod mask;
/// A cut download kept between tries so it can carry on.
mod partial;
mod result_image;
/// Media identified by its first bytes; shared with the adapters that check what they send or
/// receive.
pub mod sniff;
mod store;
mod url_rules;

pub use image_size::image_size;
pub use result_image::{MAX_RESULT_PIXELS, decoded_result};
pub use store::{MediaStore, MediaStoreConfig};

/// The link rules, for other downloaders that must follow the same ones.
pub mod links {
    pub use crate::url_rules::{check, check_resolved, is_loopback, redirect_target};
}

use craft_ai_core::{
    AiError, CostEstimate, Event, JobId, JobSnapshot, ModelId, ModelInfo, Payload, PayloadHandle,
    Provider, Request,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Most distinct links one poll copies. A provider that answers with a flood of links (or one
/// link many times) cannot turn a job into unbounded downloads and disk writes; past this the
/// outputs keep their links and a notice says so.
pub const MAX_SAVES_PER_POLL: usize = 16;

/// A provider whose hosted outputs are copied to disk as they arrive.
pub struct PersistingProvider {
    inner: Arc<dyn Provider>,
    store: Arc<MediaStore>,
}

impl PersistingProvider {
    /// Wraps `inner` so its remote outputs are copied into `store` as polls return them.
    pub fn new(inner: Arc<dyn Provider>, store: Arc<MediaStore>) -> Self {
        Self { inner, store }
    }
}

impl Provider for PersistingProvider {
    /// The inner provider's models.
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        self.inner.models()
    }

    /// The inner provider's cost estimate.
    fn estimate(
        &self,
        model: &ModelId,
        request: &Request,
    ) -> Result<Option<CostEstimate>, AiError> {
        self.inner.estimate(model, request)
    }

    /// Passes the job straight to the inner provider.
    fn submit(&self, model: &ModelId, request: &Request) -> Result<JobId, AiError> {
        self.inner.submit(model, request)
    }

    /// Polls the inner provider and swaps each remote output link for a local copy. A link
    /// already copied in this poll reuses that copy, and copying stops once `MAX_SAVES_PER_POLL`
    /// copies are saved, with one notice. A failed copy keeps the link, with its known expiry set on the handle, and adds a notice that says why and, if the
    /// link is not already expired, when it stops working.
    fn poll(&self, job: &JobId) -> Result<JobSnapshot, AiError> {
        let mut snapshot = self.inner.poll(job)?;
        let mut notices = Vec::new();
        // The same link is fetched once and its copy reused.
        let mut saved: HashMap<String, PayloadHandle> = HashMap::new();
        let mut over_limit = false;
        for event in &mut snapshot.events {
            let Event::Output(output) = event else {
                continue;
            };
            let Payload::Handle(handle) = &output.payload else {
                continue;
            };
            if !MediaStore::is_remote(handle) {
                continue;
            }
            if let Some(copy) = saved.get(&handle.uri) {
                output.payload = Payload::Handle(copy.clone());
                continue;
            }
            if saved.len() >= MAX_SAVES_PER_POLL {
                over_limit = true;
                // Left as a link, so the app needs to know how long it lasts.
                let at = known_expiry(handle);
                if let Payload::Handle(skipped) = &mut output.payload {
                    skipped.expires_at = at;
                }
                continue;
            }
            match self.store.save(handle) {
                Ok(copy) => {
                    saved.insert(handle.uri.clone(), copy.clone());
                    output.payload = Payload::Handle(copy);
                }
                Err(error) => {
                    let mut message =
                        format!("No local copy of the result was saved: {}", error.message);
                    if let Some(at) = known_expiry(handle) {
                        if !expiry::clearly_expired(at, expiry::now_unix()) {
                            message.push_str(&format!(
                                ". The link stops working at {}",
                                expiry::utc_text(at)
                            ));
                        }
                        if let Payload::Handle(kept) = &mut output.payload {
                            kept.expires_at = Some(at);
                        }
                    }
                    notices.push(Event::Notice { message });
                }
            }
        }
        if over_limit {
            notices.push(Event::Notice {
                message: format!(
                    "No local copy was saved for results past the first {MAX_SAVES_PER_POLL}."
                ),
            });
        }
        snapshot.events.extend(notices);
        Ok(snapshot)
    }

    /// Passes the cancel straight to the inner provider.
    fn cancel(&self, job: &JobId) -> Result<(), AiError> {
        self.inner.cancel(job)
    }
}

/// The earlier of the handle's own expiry and the one signed into its link.
fn known_expiry(handle: &PayloadHandle) -> Option<u64> {
    match url::Url::parse(&handle.uri) {
        Ok(link) => expiry::earliest(handle.expires_at, &link),
        Err(_) => handle.expires_at,
    }
}
