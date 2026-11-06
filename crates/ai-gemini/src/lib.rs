//! Google Gemini image models ("Nano Banana") through the Gemini API
//! (`generativelanguage.googleapis.com`) with an AI Studio API key. This is not Vertex AI: a
//! Vertex express key is `craft-ai-vertex`.
//!
//! It serves `image.inpaint` (Generative fill) through the Interactions endpoint. Gemini takes no
//! mask parameter, so this is prompt-guided editing: the mask is shown to the model as a second
//! picture with an instruction that explains it (see `fill`). The model may change pixels outside
//! the mask; the caller keeps only the masked part of the result.
//!
//! The key goes only in the `x-goog-api-key` header, never in a URL. Testing a connection lists the
//! account's models (`GET /models`), which is free; the offered list is this crate's own, because
//! the Interactions model names are not all in that listing.
//!
//! A job runs on its own thread so it can be polled and cancelled. Cancelling stops waiting for
//! the answer; a request already sent may still be billed by Google.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
mod fill;
mod registry;

pub use fill::fill_instruction;
use registry::{Jobs, Slot, lock};

use craft_ai_core::{
    AiError, ErrorKind, Event, InputRole, JobId, JobSnapshot, JobState, ModelId, ModelInfo, Output,
    Payload, Provider, Request, SecretString, capability::CapabilityId, endpoint::Endpoint,
    model::CapabilitySpec, payload::Limits,
};
use serde_json::Value;
use std::{
    collections::HashMap,
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

/// The official Gemini API address.
pub const OFFICIAL: &str = "https://generativelanguage.googleapis.com/v1beta";
/// The model offered first.
pub const DEFAULT_MODEL: &str = "gemini-nano-banana-2.1";

/// The largest answer read: a 4K picture in base64 inside JSON fits easily.
const MAX_REPLY: u64 = 48 * 1024 * 1024;
/// The most of an error answer read, for its message.
const MAX_ERROR_REPLY: u64 = 64 * 1024;
/// Requests in flight to Google at once, cancelled ones included (see [`Permit`]), and jobs
/// remembered in all.
const MAX_RUNNING: usize = 32;

#[derive(Clone, Debug)]
pub struct GeminiConfig {
    pub base_url: String,
    pub api_key: SecretString,
    pub connect_timeout: Duration,
    /// A Pro model at 4K can take well over a minute.
    pub request_timeout: Duration,
}
impl GeminiConfig {
    /// A config for `base_url` and `api_key`, with a 10 s connect and a 240 s request timeout.
    pub fn new(base_url: impl Into<String>, api_key: SecretString) -> Self {
        Self {
            base_url: base_url.into(),
            api_key,
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(240),
        }
    }
}

/// A place among the [`MAX_RUNNING`] requests in flight, held by the worker thread for its whole
/// life. Cancelling ends a job but not its request, so the place is only given back when the worker
/// exits, or when the worker never starts.
struct Permit(Arc<AtomicUsize>);
impl Permit {
    /// Takes a place, or `None` when [`MAX_RUNNING`] requests are already in flight.
    fn acquire(in_flight: &Arc<AtomicUsize>) -> Option<Self> {
        // Over the limit, dropping the permit takes the count back down. Submits racing at the
        // limit may refuse one too many, never admit one too many.
        let permit = Self(Arc::clone(in_flight));
        (in_flight.fetch_add(1, Ordering::AcqRel) < MAX_RUNNING).then_some(permit)
    }
}
impl Drop for Permit {
    /// Gives the place back.
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub struct GeminiProvider {
    base: String,
    key: SecretString,
    agent: ureq::Agent,
    jobs: Jobs,
    in_flight: Arc<AtomicUsize>,
    sequence: AtomicU64,
}
impl std::fmt::Debug for GeminiProvider {
    /// Prints the type name only, so the key can never reach a log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GeminiProvider")
    }
}

impl GeminiProvider {
    /// Builds the adapter. Refuses an empty or multi-line key, and any address other than the
    /// official one or a local test server (see [`address_allowed`](Self::address_allowed)).
    pub fn new(config: GeminiConfig) -> Result<Self, AiError> {
        if config.api_key.is_empty()
            || config
                .api_key
                .expose()
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(AiError::invalid("a single-line Gemini API key is required"));
        }
        let base = config.base_url.trim_end_matches('/');
        Endpoint::parse(base).map_err(|_| AiError::invalid("invalid Gemini API address"))?;
        if !Self::address_allowed(base) {
            return Err(AiError::invalid(
                "the Gemini API must use its official address",
            ));
        }
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .tls_config(craft_ai_http::tls_config())
                .proxy(craft_ai_http::proxy_for(base))
                .http_status_as_error(false)
                .max_redirects(0)
                .max_redirects_will_error(false)
                .timeout_connect(Some(config.connect_timeout))
                .timeout_global(Some(config.request_timeout))
                .build(),
        );
        Ok(Self {
            base: base.to_owned(),
            key: config.api_key,
            agent,
            jobs: Arc::new(Mutex::new(HashMap::new())),
            in_flight: Arc::new(AtomicUsize::new(0)),
            sequence: AtomicU64::new(0),
        })
    }

    /// Whether [`new`](Self::new) accepts `base_url`: the official address or a test server on this
    /// computer, with no query or fragment. It reads no key and contacts nothing, so the client's
    /// support check asks it before anything is built.
    pub fn address_allowed(base_url: &str) -> bool {
        let base = base_url.trim_end_matches('/');
        !base.contains(['?', '#'])
            && Endpoint::parse(base).is_ok_and(|e| e.is_loopback() || base == OFFICIAL)
    }

    /// Whether [`submit`](Provider::submit) accepts `model`: a listed model, or any well-formed
    /// `gemini-…-image…` id, so a newer model works without an update to this crate.
    pub fn model_allowed(model: &str) -> bool {
        fill::model_allowed(model)
    }

    /// The most reference pictures one fill may carry after the source and the mask. `submit`
    /// refuses more before anything is sent; the client's route check reads the same number.
    pub const MAX_REFERENCES: usize = fill::REFERENCES.max_count;

    /// This crate's own model list, each offered for Generative fill (see [`fill_spec`]).
    fn catalog() -> Vec<ModelInfo> {
        fill::MODELS
            .iter()
            .filter_map(|(id, label)| Some(ModelInfo::new(ModelId::new(*id).ok()?, *label)))
            .filter_map(|m| Some(m.with(fill_spec()?)))
            .collect()
    }
}

/// Source and mask required; the prompt may be left out or empty (fill from the surroundings).
/// Up to [`GeminiProvider::MAX_REFERENCES`] reference pictures may follow; none are required.
fn fill_spec() -> Option<CapabilitySpec> {
    Some(
        CapabilitySpec::new(CapabilityId::parse("image.inpaint").ok()?)
            .accepting(InputRole::Source, 1)
            .requiring(InputRole::Source)
            .accepting(InputRole::Mask, 1)
            .requiring(InputRole::Mask)
            .accepting(InputRole::Prompt, 1)
            .accepting(
                InputRole::Reference,
                u32::try_from(GeminiProvider::MAX_REFERENCES).ok()?,
            ),
    )
}

/// Reads at most `max` bytes of a body; more is `None`.
fn read_bounded(body: ureq::Body, max: u64) -> Result<Option<Vec<u8>>, AiError> {
    let mut bytes = Vec::new();
    body.into_reader()
        .take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| AiError::new(ErrorKind::Network, "Gemini reply ended early"))?;
    Ok((bytes.len() as u64 <= max).then_some(bytes))
}

/// A refused request as an error: the kind from the status (and, for 400, whether Google says the
/// key is bad), with Google's message, the key scrubbed out and cut short.
fn status_error(status: u16, retry_after: Option<u64>, body: &[u8], key: &SecretString) -> AiError {
    // Some errors (402 when prepaid credit runs out) come wrapped in a one-element array.
    let error = serde_json::from_slice::<Value>(body).ok().and_then(|v| {
        let v = match v {
            Value::Array(mut items) if !items.is_empty() => items.swap_remove(0),
            v => v,
        };
        v.get("error").cloned()
    });
    let field = |name: &str| {
        error
            .as_ref()
            .and_then(|e| e.get(name))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let (message, reason) = (field("message"), field("status"));
    let bad_key =
        message.contains("API key") || message.contains("API_KEY") || reason == "UNAUTHENTICATED";
    let kind = match status {
        401 | 403 => ErrorKind::Auth,
        400 if bad_key => ErrorKind::Auth,
        402 => ErrorKind::Quota,
        429 => ErrorKind::RateLimited,
        400 | 404 | 405 | 413 | 422 => ErrorKind::InvalidRequest,
        _ => ErrorKind::Provider,
    };
    let detail = if message.is_empty() { reason } else { message };
    // Scrub the key from the whole message before cutting it short, so a key that straddles
    // the cut cannot leave a fragment behind.
    let detail = AiError::redacted(kind, &detail, &[key]).message;
    let text = if detail.is_empty() {
        format!("Gemini answered {status}")
    } else {
        format!("Gemini answered {status}: {}", fill::clip(&detail, 300))
    };
    let e = AiError::new(kind, text);
    match retry_after {
        Some(secs) if status == 429 => e.with_retry_after(secs),
        _ => e,
    }
}

/// Sends a request and returns the JSON answer, or the error a refusal maps to.
fn call(
    response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    key: &SecretString,
) -> Result<Value, AiError> {
    let response =
        response.map_err(|_| AiError::new(ErrorKind::Network, "could not reach the Gemini API"))?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok());
        let body = read_bounded(response.into_body(), MAX_ERROR_REPLY)
            .ok()
            .flatten()
            .unwrap_or_default();
        return Err(status_error(status, retry_after, &body, key));
    }
    let bytes = read_bounded(response.into_body(), MAX_REPLY)?
        .ok_or_else(|| AiError::new(ErrorKind::Provider, "Gemini reply is too large"))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| AiError::new(ErrorKind::Provider, "Gemini returned invalid JSON"))
}

/// One fill, start to finish: the picture, or why there is none.
fn run_fill(
    agent: &ureq::Agent,
    url: &str,
    key: &SecretString,
    body: &[u8],
) -> Result<Payload, AiError> {
    let reply = call(
        agent
            .post(url)
            .header("x-goog-api-key", key.expose())
            .content_type("application/json")
            .send(body),
        key,
    )?;
    match fill::read_answer(&reply)? {
        fill::Answer::Image(media, bytes) => Payload::inline(media, bytes),
        fill::Answer::Text(text) if text.is_empty() => Err(AiError::new(
            ErrorKind::Provider,
            "Gemini returned no image",
        )),
        fill::Answer::Text(text) => {
            // Scrub before cutting, as in `status_error`.
            let text = AiError::redacted(ErrorKind::Provider, &text, &[key]).message;
            Err(AiError::new(
                ErrorKind::Provider,
                format!("Gemini returned no image: {}", fill::clip(&text, 300)),
            ))
        }
    }
}

impl Provider for GeminiProvider {
    /// Checks the key with one free model listing, then offers this crate's own model list.
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        // Listing models is free and checks the key; one entry is enough to know it works.
        call(
            self.agent
                .get(format!("{}/models?pageSize=1", self.base))
                .header("x-goog-api-key", self.key.expose())
                .call(),
            &self.key,
        )?;
        Ok(Self::catalog())
    }

    /// Checks the model, job and inputs, makes room in the registry (see `registry::admit`), and
    /// starts the fill on its own thread. Refuses before anything is sent.
    fn submit(&self, model: &ModelId, request: &Request) -> Result<JobId, AiError> {
        if !fill::model_allowed(model.as_str()) {
            return Err(AiError::unsupported(format!(
                "{} is not a Gemini image model",
                model.as_str()
            )));
        }
        if request.capability.as_str() != "image.inpaint" {
            return Err(AiError::unsupported(
                "this Gemini adapter only does Generative fill (image.inpaint)",
            ));
        }
        if request.outputs != 1 || request.stream {
            return Err(AiError::unsupported(
                "Gemini fill makes one picture per request and does not stream",
            ));
        }
        if let Some(other) = request.inputs.iter().find(|i| {
            !matches!(
                i.role,
                InputRole::Source | InputRole::Mask | InputRole::Prompt | InputRole::Reference
            )
        }) {
            return Err(AiError::unsupported(format!(
                "Gemini fill takes no {:?} input",
                other.role
            )));
        }
        for role in [InputRole::Source, InputRole::Mask, InputRole::Prompt] {
            if request.inputs.iter().filter(|i| i.role == role).count() > 1 {
                return Err(AiError::unsupported(format!(
                    "Gemini fill takes one {role:?} input"
                )));
            }
        }
        for input in &request.inputs {
            input.payload.check(&Limits::default())?;
        }
        let body = serde_json::to_vec(&fill::body(model.as_str(), request)?)
            .map_err(|_| AiError::new(ErrorKind::Internal, "could not encode Gemini request"))?;
        let id = JobId::new(format!(
            "gemini-{}",
            self.sequence.fetch_add(1, Ordering::Relaxed)
        ))
        .map_err(|_| AiError::new(ErrorKind::Internal, "cannot make Gemini job ID"))?;
        let permit = Permit::acquire(&self.in_flight).ok_or_else(|| {
            AiError::new(
                ErrorKind::RateLimited,
                "too many Gemini fills are running; wait for one to finish",
            )
        })?;
        {
            let mut jobs = lock(&self.jobs)?;
            registry::admit(&mut jobs, Instant::now())?;
            jobs.insert(id.clone(), Slot::running());
        }
        let (agent, key, jobs, job) = (
            self.agent.clone(),
            self.key.clone(),
            Arc::clone(&self.jobs),
            id.clone(),
        );
        let url = format!("{}/interactions", self.base);
        let spawned = std::thread::Builder::new()
            .name("craft-ai-gemini".into())
            .spawn(move || {
                let _permit = permit;
                let result = run_fill(&agent, &url, &key, &body);
                let Ok(mut jobs) = lock(&jobs) else { return };
                // A cancelled (or forgotten) job keeps its state; the answer is dropped.
                let Some(slot) = jobs.get_mut(&job).filter(|s| !s.state.is_terminal()) else {
                    return;
                };
                match result {
                    Ok(payload) => {
                        slot.events.push(Event::Output(Output {
                            payload,
                            label: None,
                        }));
                        slot.finish(JobState::Succeeded, Instant::now());
                    }
                    Err(error) => slot.finish(JobState::Failed { error }, Instant::now()),
                }
            });
        if spawned.is_err() {
            lock(&self.jobs)?.remove(&id);
            return Err(AiError::new(
                ErrorKind::Internal,
                "could not start the Gemini request",
            ));
        }
        Ok(id)
    }

    /// Hands out the job's state and any events not yet collected, and marks a terminal state as
    /// delivered.
    fn poll(&self, id: &JobId) -> Result<JobSnapshot, AiError> {
        let mut jobs = lock(&self.jobs)?;
        let slot = jobs
            .get_mut(id)
            .ok_or_else(|| AiError::invalid("unknown Gemini job"))?;
        let mut snapshot = JobSnapshot::new(slot.state.clone());
        snapshot.events = std::mem::take(&mut slot.events);
        slot.terminal_delivered |= slot.state.is_terminal();
        Ok(snapshot)
    }

    /// Stops waiting for the answer and records when the job ended, so its age limit applies. A
    /// request already sent may still be billed.
    fn cancel(&self, id: &JobId) -> Result<(), AiError> {
        let mut jobs = lock(&self.jobs)?;
        let slot = jobs
            .get_mut(id)
            .ok_or_else(|| AiError::invalid("unknown Gemini job"))?;
        if !slot.state.is_terminal() {
            slot.finish(JobState::Cancelled, Instant::now());
        }
        Ok(())
    }
}

#[cfg(test)]
mod registry_limits_tests;
#[cfg(test)]
mod registry_tests;
