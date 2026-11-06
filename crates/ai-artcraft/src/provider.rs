//! API-key generation and polling, not subscription entitlement.
use crate::http::Http;
use craft_ai_core::{
    AiError, ErrorKind, Event, InputRole, JobId, JobSnapshot, JobState, ModelId, ModelInfo, Output,
    Payload, Provider, Request, SecretString, endpoint::Endpoint, payload::Limits,
};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{collections::HashMap, sync::Mutex, time::Duration};

#[derive(Clone, Debug)]
pub struct ArtCraftConfig {
    pub base_url: String,
    pub api_key: SecretString,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    /// Limits tracked jobs and reserves a slot before a paid request is sent.
    pub max_active_jobs: usize,
}
impl ArtCraftConfig {
    /// A config for `base_url` with `api_key`, the default timeouts and at most 16 tracked jobs.
    /// Nothing is checked until the adapter is built.
    pub fn new(base_url: impl Into<String>, api_key: SecretString) -> Self {
        Self {
            base_url: base_url.into(),
            api_key,
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(60),
            max_active_jobs: 16,
        }
    }
}
/// Most jobs remembered, finished ones included. Oldest finished jobs are dropped first.
const KEEP_JOBS: usize = 128;
/// Polls that may fail to fetch a finished job's result for a passing reason (network, busy
/// service) before the job is given up as failed, so it stops holding an active-job slot.
const RESULT_TRIES: u8 = 5;

struct Job {
    remote: String,
    video: bool,
    inline_image: bool,
    /// Failed fetches of this finished job's result so far.
    result_failures: u8,
    snapshot: JobSnapshot,
}
/// Holds a slot for a request that has been sent but has no job token yet, so concurrent
/// submits cannot all pass the limit before any of them is recorded.
struct Reservation<'a>(&'a AtomicUsize);
impl Drop for Reservation<'_> {
    /// Frees the slot when the submit ends, whether it recorded a job or failed.
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
pub struct ArtCraftProvider {
    http: Http,
    jobs: Mutex<HashMap<JobId, Job>>,
    pending: AtomicUsize,
    max_active: usize,
}
impl std::fmt::Debug for ArtCraftProvider {
    /// Names the adapter only, so a debug print never shows the key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ArtCraftProvider(Omni API key)")
    }
}
impl ArtCraftProvider {
    /// Whether [`new`](Self::new) accepts `base_url`: the official address or a test server on this
    /// computer, with no query or fragment. It reads no key and contacts nothing, so the client's
    /// support check asks it before anything is built.
    pub fn address_allowed(base_url: &str) -> bool {
        let base = base_url.trim_end_matches('/');
        !base.contains(['?', '#'])
            && Endpoint::parse(base)
                .is_ok_and(|e| e.is_loopback() || base == "https://api.storyteller.ai")
    }

    /// Checks the key is one line and the address is one [`address_allowed`](Self::address_allowed)
    /// accepts, then builds the adapter. It contacts nothing.
    pub fn new(config: ArtCraftConfig) -> Result<Self, AiError> {
        if config.api_key.is_empty()
            || config
                .api_key
                .expose()
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(AiError::invalid(
                "a single-line ArtCraft API key is required",
            ));
        }
        let base = config.base_url.trim_end_matches('/');
        Endpoint::parse(base).map_err(|_| AiError::invalid("invalid ArtCraft Omni API address"))?;
        if !Self::address_allowed(base) {
            return Err(AiError::invalid(
                "ArtCraft Omni API must use its official address",
            ));
        }
        Ok(Self {
            http: Http::new(
                base.to_owned(),
                config.api_key,
                config.connect_timeout,
                config.request_timeout,
            ),
            jobs: Mutex::new(HashMap::new()),
            pending: AtomicUsize::new(0),
            max_active: config.max_active_jobs.max(1),
        })
    }
    /// Models supported by this adapter; listing them does not verify an account.
    pub fn documented_models() -> Vec<ModelInfo> {
        crate::models::documented()
    }
    /// A finished job whose result could not be fetched. A passing failure is returned so the
    /// caller polls again, up to [`RESULT_TRIES`]; any other failure, or one too many, ends the job
    /// here as failed. The remote job is not cancelled (it has already finished); it is only
    /// forgotten as active, so it no longer counts against `max_active_jobs`.
    fn result_failed(&self, id: &JobId, error: AiError) -> Result<JobSnapshot, AiError> {
        let mut jobs = self.lock()?;
        let Some(job) = jobs.get_mut(id) else {
            return Err(error);
        };
        if job.snapshot.state.is_terminal() {
            return Ok(JobSnapshot::new(job.snapshot.state.clone()));
        }
        job.result_failures = job.result_failures.saturating_add(1);
        let passing = matches!(error.kind, ErrorKind::Network | ErrorKind::RateLimited);
        if passing && job.result_failures < RESULT_TRIES {
            return Err(error);
        }
        job.snapshot = JobSnapshot::new(JobState::Failed { error });
        Ok(job.snapshot.clone())
    }
    /// The job registry, or an internal error when a panic left its lock poisoned.
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<JobId, Job>>, AiError> {
        self.jobs
            .lock()
            .map_err(|_| AiError::new(ErrorKind::Internal, "ArtCraft job registry unavailable"))
    }
}
impl Provider for ArtCraftProvider {
    /// Always refuses: Omni has no free way to test a key, so saved models stay unverified until a
    /// real job runs. Contacts nothing.
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        Err(AiError::unsupported(
            "ArtCraft Omni API has no documented free key test; saved models are unverified until a job runs",
        ))
    }
    /// Checks the request against the documented model, reserves a job slot, uploads the fill's
    /// source, mask and reference pictures in that order when it is a fill, then asks Omni to
    /// generate. Nothing is sent for a refused request.
    fn submit(&self, model: &ModelId, request: &Request) -> Result<JobId, AiError> {
        let info = Self::documented_models()
            .into_iter()
            .find(|m| &m.id == model)
            .ok_or_else(|| {
                AiError::unsupported(format!("ArtCraft Omni API model {model} is not configured"))
            })?;
        let spec = info.spec_for(&request.capability).ok_or_else(|| {
            AiError::unsupported(format!("{model} cannot do {}", request.capability))
        })?;
        request.validate(spec, &Limits::default())?;
        if !request.params.is_empty() {
            return Err(AiError::unsupported(
                "ArtCraft Omni API options are not mapped yet",
            ));
        }
        let fill = if request.capability.as_str() == "image.inpaint" {
            let max_references = spec
                .accepts
                .get(&InputRole::Reference)
                .map_or(0, |n| usize::try_from(*n).unwrap_or(0));
            Some(crate::fill::prepare(request, max_references)?)
        } else {
            None
        };
        let prompt = match request.inputs.iter().find(|i| i.role == InputRole::Prompt) {
            None => None,
            Some(input) => match &input.payload {
                Payload::Text { text } => Some(text.as_str()),
                // A prompt that is not text is a mistake, never an empty prompt.
                _ => return Err(AiError::invalid("the ArtCraft Omni prompt must be text")),
            },
        };
        // A fill may leave the prompt out: the shared instruction then asks Omni to remove what the
        // mask covers. Generation always needs words.
        let prompt = match prompt {
            Some(text) if fill.is_some() || !text.trim().is_empty() => text,
            None if fill.is_some() => "",
            _ => return Err(AiError::invalid("ArtCraft Omni API needs a text prompt")),
        };
        let video = model.as_str() == "seedance_2p0";
        // Reserve before sending: a paid job that cannot be tracked must never be created.
        let _slot = {
            let jobs = self.lock()?;
            let active = jobs
                .values()
                .filter(|j| !j.snapshot.state.is_terminal())
                .count();
            if active + self.pending.load(Ordering::SeqCst) >= self.max_active {
                return Err(AiError::new(
                    ErrorKind::Quota,
                    "too many ArtCraft jobs are running; wait for one to finish",
                ));
            }
            self.pending.fetch_add(1, Ordering::SeqCst);
            Reservation(&self.pending)
        };
        let mut body = json!({"model":model.as_str(),"prompt":prompt,"idempotency_token":uuid::Uuid::new_v4().to_string()});
        if let Some(fill) = &fill {
            let mut tokens = vec![
                self.http.upload_png(fill.source)?,
                self.http.upload_png(&fill.mask)?,
            ];
            for reference in &fill.references {
                tokens.push(self.http.upload_png(reference.data)?);
            }
            body["image_media_tokens"] = json!(tokens);
            body["image_batch_count"] = json!(1);
            body["prompt"] = json!(craft_ai_media::mask::fill_instruction_with_references(
                prompt,
                fill.references.len()
            ));
        }
        let response = self.http.post(
            if video {
                "/v1/omni_api/generate/video"
            } else {
                "/v1/omni_api/generate/image"
            },
            &body,
        )?;
        if response.get("success").and_then(Value::as_bool) != Some(true) {
            return Err(AiError::new(
                ErrorKind::Provider,
                "ArtCraft Omni API did not accept the generation",
            ));
        }
        let remote = response
            .get("inference_job_token")
            .and_then(Value::as_str)
            .filter(|s| {
                s.starts_with("jinf_")
                    && s.len() <= 128
                    && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            })
            .ok_or_else(|| {
                AiError::new(
                    ErrorKind::Provider,
                    "ArtCraft Omni API returned an invalid job token",
                )
            })?;
        let id = JobId::new(format!("artcraft-{remote}")).map_err(|_| {
            AiError::new(
                ErrorKind::Provider,
                "ArtCraft job token cannot be represented",
            )
        })?;
        let mut jobs = self.lock()?;
        while jobs.len() >= KEEP_JOBS {
            match jobs.iter().find(|(_, j)| j.snapshot.state.is_terminal()) {
                Some((old, _)) => {
                    let old = old.clone();
                    jobs.remove(&old);
                }
                None => break,
            }
        }
        jobs.insert(
            id.clone(),
            Job {
                remote: remote.to_owned(),
                video,
                inline_image: fill.is_some(),
                result_failures: 0,
                snapshot: JobSnapshot::new(JobState::Queued),
            },
        );
        Ok(id)
    }
    /// Asks Omni for the job's state. A finished fill's picture is fetched and handed over once; a
    /// finished job is not asked about again, and its stored state keeps no picture bytes.
    fn poll(&self, id: &JobId) -> Result<JobSnapshot, AiError> {
        let (remote, video, inline_image, prior) = {
            let jobs = self.lock()?;
            let job = jobs
                .get(id)
                .ok_or_else(|| AiError::invalid("unknown ArtCraft job"))?;
            if job.snapshot.state.is_terminal() {
                let mut stable = job.snapshot.clone();
                stable.events.clear();
                return Ok(stable);
            }
            (
                job.remote.clone(),
                job.video,
                job.inline_image,
                job.snapshot.state.clone(),
            )
        };
        let reply = self
            .http
            .get(&format!("/v1/omni_api/job_status/job/{remote}"))?;
        if reply.get("success").and_then(Value::as_bool) != Some(true)
            || reply.pointer("/state/job_token").and_then(Value::as_str) != Some(remote.as_str())
        {
            return Err(AiError::new(
                ErrorKind::Provider,
                "ArtCraft Omni API returned the wrong job",
            ));
        }
        let status = reply
            .pointer("/state/status/status")
            .and_then(Value::as_str)
            .unwrap_or("");
        let percent = reply
            .pointer("/state/status/progress_percentage")
            .and_then(Value::as_u64)
            .and_then(|n| u8::try_from(n).ok())
            .filter(|n| *n <= 100);
        let mut snap = match status {
            "pending" => JobSnapshot::new(prior.clone()),
            "started" | "attempt_failed" => JobSnapshot::new(JobState::Running {
                percent: match &prior {
                    JobState::Running {
                        percent: Some(previous),
                    } => Some(percent.unwrap_or(*previous).max(*previous)),
                    _ => percent,
                },
            }),
            "complete_success" => {
                let uri = reply
                    .pointer("/state/maybe_result/media_links/cdn_url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        AiError::new(ErrorKind::Provider, "ArtCraft job result has no CDN URL")
                    })?;
                let payload = match crate::result::payload(&self.http, uri, video, inline_image) {
                    Ok(payload) => payload,
                    Err(error) => return self.result_failed(id, error),
                };
                let mut done = JobSnapshot::new(JobState::Succeeded);
                done.events.push(Event::Output(Output {
                    payload,
                    label: None,
                }));
                done
            }
            "complete_failure" | "dead" => JobSnapshot::new(JobState::Failed {
                error: AiError::new(ErrorKind::Provider, format!("ArtCraft job ended: {status}")),
            }),
            "cancelled_by_user" | "cancelled_by_system" => JobSnapshot::new(JobState::Cancelled),
            _ => {
                return Err(AiError::new(
                    ErrorKind::Provider,
                    "ArtCraft returned an unknown job state",
                ));
            }
        };
        let mut jobs = self.lock()?;
        if let Some(job) = jobs.get_mut(id) {
            if job.snapshot.state.is_terminal() {
                snap = job.snapshot.clone();
                snap.events.clear();
            } else if job.snapshot.state.can_become(&snap.state) {
                // Keep the state and usage only: the output was handed over in this answer, and a
                // later poll of a finished job returns no events, so its bytes are never copied again.
                job.snapshot = JobSnapshot {
                    state: snap.state.clone(),
                    events: Vec::new(),
                    usage: snap.usage.clone(),
                };
            } else {
                snap = job.snapshot.clone();
                snap.events.clear();
            }
        }
        Ok(snap)
    }
    fn cancel(&self, id: &JobId) -> Result<(), AiError> {
        let jobs = self.lock()?;
        match jobs.get(id) {
            None => Err(AiError::invalid("unknown ArtCraft job")),
            Some(job) if job.snapshot.state.is_terminal() => Ok(()),
            Some(_) => Err(AiError::unsupported(
                "ArtCraft Omni API has no documented job cancellation route",
            )),
        }
    }
}
