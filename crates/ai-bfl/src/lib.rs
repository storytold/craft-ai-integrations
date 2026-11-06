//! BFL FLUX 3 image/video jobs and FLUX.1 Fill [pro] inpainting via the official asynchronous API.
//! A task is polled at the `polling_url` BFL returns when that is https on `bfl.ai` (BFL requires it
//! on the global address), otherwise at the configured origin; see `polling`. FLUX 3 media are
//! short-lived remote handles; callers must persist them if they need durable assets. A fill is
//! downloaded as soon as it is ready (BFL's links last ten minutes) and returned inline.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]
mod fill;
mod http;
mod polling;
use craft_ai_core::payload::DEFAULT_MAX_INLINE_BYTES;
use craft_ai_core::{
    AiError, ErrorKind, Event, InputRole, JobId, JobSnapshot, JobState, ModelId, ModelInfo, Output,
    Payload, Provider, Request, SecretString, capability::CapabilityId, endpoint::Endpoint,
    model::CapabilitySpec, payload::Limits,
};
use http::{Http, encode_query};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct BflConfig {
    pub base_url: String,
    pub api_key: SecretString,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    /// How long a task may stay "Task not found" (over at least ten polls) before the job fails.
    pub not_found_grace: Duration,
}
impl BflConfig {
    /// A config for `base_url` with `api_key` and the default timeouts. Nothing is checked until
    /// the adapter is built.
    pub fn new(base_url: impl Into<String>, api_key: SecretString) -> Self {
        Self {
            base_url: base_url.into(),
            api_key,
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(60),
            not_found_grace: Duration::from_secs(60),
        }
    }
}

/// The official global address and BFL's two regional ones.
const OFFICIAL: &[&str] = &[
    "https://api.bfl.ai/v1",
    "https://api.eu.bfl.ai/v1",
    "https://api.us.bfl.ai/v1",
];

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Image,
    Video,
    Fill,
}

struct Job {
    remote_id: String,
    /// Where the task is read: the checked `polling_url`, or the configured address.
    poll_url: String,
    kind: Kind,
    snapshot: JobSnapshot,
    /// Consecutive "Task not found" answers, and when the first of them came.
    not_found: Option<(u32, Instant)>,
}

pub struct BflProvider {
    http: Http,
    base: Endpoint,
    loopback: bool,
    not_found_grace: Duration,
    jobs: Mutex<HashMap<JobId, Job>>,
}
impl std::fmt::Debug for BflProvider {
    /// Names the adapter only, so a debug print never shows the key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BflProvider")
    }
}
impl BflProvider {
    /// Whether [`new`](Self::new) accepts `base_url`: the official address or a test server on this
    /// computer, with no query or fragment. It reads no key and contacts nothing, so the client's
    /// support check asks it before anything is built.
    pub fn address_allowed(base_url: &str) -> bool {
        let base = base_url.trim_end_matches('/');
        !base.contains(['?', '#'])
            && Endpoint::parse(base).is_ok_and(|e| e.is_loopback() || OFFICIAL.contains(&base))
    }

    /// Checks the key is one line and the address is one [`address_allowed`](Self::address_allowed)
    /// accepts, then builds the adapter. It contacts nothing.
    pub fn new(config: BflConfig) -> Result<Self, AiError> {
        if config.api_key.is_empty()
            || config
                .api_key
                .expose()
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(AiError::invalid("a single-line BFL API key is required"));
        }
        let base = config.base_url.trim_end_matches('/');
        let endpoint =
            Endpoint::parse(base).map_err(|_| AiError::invalid("invalid BFL API address"))?;
        if !Self::address_allowed(base) {
            return Err(AiError::invalid("BFL must use its official API address"));
        }
        Ok(Self {
            http: Http::new(
                base.to_owned(),
                config.api_key,
                config.connect_timeout,
                config.request_timeout,
            ),
            loopback: endpoint.is_loopback(),
            base: endpoint,
            not_found_grace: config.not_found_grace,
            jobs: Mutex::new(HashMap::new()),
        })
    }
    /// The models this adapter serves, and what each can do.
    pub fn catalog() -> Vec<ModelInfo> {
        let mut models: Vec<ModelInfo> = [
            ("flux-3-image", "FLUX 3 image", "image.generate"),
            ("flux-3-video", "FLUX 3 video", "video.generate"),
        ]
        .into_iter()
        .filter_map(|(id, label, cap)| {
            Some(
                ModelInfo::new(ModelId::new(id).ok()?, label).with(
                    CapabilitySpec::new(CapabilityId::parse(cap).ok()?)
                        .accepting(InputRole::Prompt, 1)
                        .requiring(InputRole::Prompt),
                ),
            )
        })
        .collect();
        if let (Ok(id), Ok(cap)) = (
            ModelId::new(fill::MODEL),
            CapabilityId::parse("image.inpaint"),
        ) {
            models.push(
                ModelInfo::new(id, "FLUX.1 Fill [pro]").with(
                    CapabilitySpec::new(cap)
                        .accepting(InputRole::Source, 1)
                        .requiring(InputRole::Source)
                        .accepting(InputRole::Mask, 1)
                        .requiring(InputRole::Mask)
                        .accepting(InputRole::Prompt, 1)
                        .requiring(InputRole::Prompt),
                ),
            );
        }
        models
    }
    /// FLUX 3 image or video: a prompt and no options.
    fn generate_body(request: &Request, kind: Kind) -> Result<(&'static str, Value), AiError> {
        if !request.params.is_empty() {
            return Err(AiError::unsupported(
                "BFL options are not mapped yet; use provider defaults",
            ));
        }
        let prompt = request
            .inputs
            .iter()
            .find(|i| i.role == InputRole::Prompt)
            .and_then(|i| match &i.payload {
                Payload::Text { text } => Some(text),
                _ => None,
            })
            .ok_or_else(|| AiError::invalid("BFL needs a text prompt"))?;
        if prompt.is_empty() {
            return Err(AiError::invalid("BFL needs a non-empty prompt"));
        }
        Ok(if kind == Kind::Video {
            ("/flux-3-video", json!({"prompt":prompt,"mode":"t2v"}))
        } else {
            ("/flux-3-image", json!({"prompt":prompt}))
        })
    }
    /// The finished fill, downloaded now because BFL's link expires. A link that is not BFL's,
    /// too large or not a picture fails the job; a dropped connection is an error the next poll
    /// retries.
    fn fill_result(&self, uri: &str) -> Result<JobSnapshot, AiError> {
        if !fill::delivery_allowed(uri, self.loopback) {
            return Ok(failed("BFL result URL is not a BFL delivery address"));
        }
        let bytes = match self.http.download(uri, DEFAULT_MAX_INLINE_BYTES) {
            Ok(bytes) => bytes,
            Err(e) if e.is_retryable() => return Err(e),
            Err(e) => return Ok(JobSnapshot::new(JobState::Failed { error: e })),
        };
        let Some(media) = fill::result_media(&bytes) else {
            return Ok(failed("the BFL result is not a PNG, JPEG or WebP image"));
        };
        let mut snap = JobSnapshot::new(JobState::Succeeded);
        snap.events.push(Event::Output(Output {
            payload: Payload::inline(media, bytes)?,
            label: None,
        }));
        Ok(snap)
    }
    /// A FLUX 3 result: the remote link, as a handle for the caller to persist.
    fn handle_result(uri: &str, kind: Kind) -> Result<JobSnapshot, AiError> {
        let endpoint = Endpoint::parse(uri)
            .map_err(|_| AiError::new(ErrorKind::Provider, "BFL result URL is invalid"))?;
        if !endpoint.https
            || endpoint.is_loopback()
            || uri.contains(['#', '?']) && uri.contains('@')
        {
            return Err(AiError::new(
                ErrorKind::Provider,
                "BFL result URL is unsafe",
            ));
        }
        let media = if kind == Kind::Video {
            "video/mp4"
        } else {
            "image/png"
        };
        let payload = Payload::handle(uri, media, None)?;
        let mut snap = JobSnapshot::new(JobState::Succeeded);
        snap.events.push(Event::Output(Output {
            payload,
            label: None,
        }));
        Ok(snap)
    }
    fn job_lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<JobId, Job>>, AiError> {
        self.jobs
            .lock()
            .map_err(|_| AiError::new(ErrorKind::Internal, "BFL job registry unavailable"))
    }
}
fn failed(message: &str) -> JobSnapshot {
    JobSnapshot::new(JobState::Failed {
        error: AiError::new(ErrorKind::Provider, message),
    })
}

impl Provider for BflProvider {
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        let value = self.http.get("/credits")?;
        if !value.get("credits").is_some_and(Value::is_number) {
            return Err(AiError::new(
                ErrorKind::Provider,
                "BFL credits reply is invalid",
            ));
        }
        Ok(Self::catalog())
    }
    /// Checks the request against the catalogue, builds the body (a fill is fully checked here),
    /// sends it and records the task with its polling address.
    fn submit(&self, model: &ModelId, request: &Request) -> Result<JobId, AiError> {
        let info = Self::catalog()
            .into_iter()
            .find(|m| &m.id == model)
            .ok_or_else(|| AiError::unsupported(format!("BFL does not serve model {model}")))?;
        let spec = info.spec_for(&request.capability).ok_or_else(|| {
            AiError::unsupported(format!("{model} cannot do {}", request.capability))
        })?;
        request.validate(spec, &Limits::default())?;
        let kind = match model.as_str() {
            fill::MODEL => Kind::Fill,
            "flux-3-video" => Kind::Video,
            _ => Kind::Image,
        };
        let (path, body) = if kind == Kind::Fill {
            (fill::PATH, fill::body(request)?)
        } else {
            Self::generate_body(request, kind)?
        };
        let reply = self.http.post(path, &body)?;
        let remote = reply
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() < 256 && !s.chars().any(char::is_control))
            .ok_or_else(|| AiError::new(ErrorKind::Provider, "BFL task ID is invalid"))?;
        let id = JobId::new(format!("bfl-{remote}"))
            .map_err(|_| AiError::new(ErrorKind::Provider, "BFL task ID cannot be represented"))?;
        let poll_url = reply
            .get("polling_url")
            .and_then(Value::as_str)
            .filter(|u| polling::polling_url_allowed(u, &self.base))
            .map_or_else(
                || {
                    self.http
                        .url(&format!("/get_result?id={}", encode_query(remote)))
                },
                str::to_owned,
            );
        let mut jobs = self.job_lock()?;
        if jobs.len() >= 128 {
            return Err(AiError::new(ErrorKind::Quota, "too many tracked BFL jobs"));
        }
        jobs.insert(
            id.clone(),
            Job {
                remote_id: remote.to_owned(),
                poll_url,
                kind,
                snapshot: JobSnapshot::new(JobState::Queued),
                not_found: None,
            },
        );
        Ok(id)
    }
    /// Asks BFL for the task at its polling address. Any found answer ends a not-found streak;
    /// a Ready fill is downloaded, and a busy download is an error to poll again.
    fn poll(&self, id: &JobId) -> Result<JobSnapshot, AiError> {
        let (remote, url, kind, prior) = {
            let jobs = self.job_lock()?;
            let job = jobs
                .get(id)
                .ok_or_else(|| AiError::invalid("unknown BFL job"))?;
            if job.snapshot.state.is_terminal() {
                let mut stable = job.snapshot.clone();
                stable.events.clear();
                return Ok(stable);
            }
            (
                job.remote_id.clone(),
                job.poll_url.clone(),
                job.kind,
                job.snapshot.state.clone(),
            )
        };
        // A 404 is the same answer as a "Task not found" status.
        let Some(reply) = self.http.poll(&url)? else {
            return self.not_found(id);
        };
        let status = reply.get("status").and_then(Value::as_str).unwrap_or("");
        if status == "Task not found" {
            return self.not_found(id);
        }
        if reply.get("id").and_then(Value::as_str) != Some(remote.as_str()) {
            return Err(AiError::new(
                ErrorKind::Provider,
                "BFL returned a different task ID",
            ));
        }
        // BFL has found the task, so the not-found streak ends now, even if fetching a Ready
        // result below fails and this poll returns an error.
        if let Some(job) = self.job_lock()?.get_mut(id) {
            job.not_found = None;
        }
        let snap = match status {
            "Pending" => JobSnapshot::new(prior),
            "Reasoning" | "Generating" => JobSnapshot::new(JobState::Running { percent: None }),
            "Ready" => {
                let field = if kind == Kind::Video {
                    "video"
                } else {
                    "sample"
                };
                let uri = reply
                    .get("result")
                    .and_then(|v| v.get(field))
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        AiError::new(ErrorKind::Provider, "BFL result has no media URL")
                    })?;
                if kind == Kind::Fill {
                    self.fill_result(uri)?
                } else {
                    Self::handle_result(uri, kind)?
                }
            }
            "Request Moderated" => failed("BFL declined the request: content moderated"),
            "Content Moderated" => failed("BFL withheld the result: content moderated"),
            "Error" | "No Card Generated" => failed(&format!("BFL task ended: {status}")),
            _ => {
                return Err(AiError::new(
                    ErrorKind::Provider,
                    "BFL returned an unknown status",
                ));
            }
        };
        self.settle(id, snap, true)
    }
    /// BFL has no cancel route: a finished job cancels quietly, a running one is refused.
    fn cancel(&self, id: &JobId) -> Result<(), AiError> {
        let jobs = self.job_lock()?;
        match jobs.get(id) {
            None => Err(AiError::invalid("unknown BFL job")),
            Some(job) if job.snapshot.state.is_terminal() => Ok(()),
            Some(_) => Err(AiError::unsupported(
                "BFL API has no task cancellation route",
            )),
        }
    }
}

impl BflProvider {
    /// Counts a "Task not found" answer and fails the job once it has lasted (see `polling`).
    /// Counting, deciding and recording happen under one lock, so a poll that found the task in
    /// between (and reset the streak) cannot be overwritten by this stale failure.
    fn not_found(&self, id: &JobId) -> Result<JobSnapshot, AiError> {
        let mut jobs = self.job_lock()?;
        let job = jobs
            .get_mut(id)
            .ok_or_else(|| AiError::invalid("unknown BFL job"))?;
        if job.snapshot.state.is_terminal() {
            return Ok(JobSnapshot::new(job.snapshot.state.clone()));
        }
        let (count, since) = job.not_found.unwrap_or((0, Instant::now()));
        let count = count.saturating_add(1);
        job.not_found = Some((count, since));
        if count >= polling::NOT_FOUND_POLLS && since.elapsed() >= self.not_found_grace {
            job.snapshot = failed("BFL could not find the job; it may have expired");
        }
        // The current state, not the one this poll started from, which may be out of date.
        Ok(JobSnapshot::new(job.snapshot.state.clone()))
    }
    /// Records `snap` unless the job already finished, and returns what the caller should see.
    /// `found` is false only for a "Task not found" answer; any other answer resets that count.
    fn settle(
        &self,
        id: &JobId,
        mut snap: JobSnapshot,
        found: bool,
    ) -> Result<JobSnapshot, AiError> {
        let mut jobs = self.job_lock()?;
        if let Some(job) = jobs.get_mut(id) {
            if found {
                job.not_found = None;
            }
            if job.snapshot.state.is_terminal() {
                snap = job.snapshot.clone();
                snap.events.clear();
            } else {
                // Keep the state only: an inline result is handed out once, not held twice.
                job.snapshot = JobSnapshot::new(snap.state.clone());
            }
        }
        Ok(snap)
    }
}
