//! Evidence of one real run through [`AiClient`](crate::AiClient): what was asked, where it went,
//! what came back and how long it took, kept as one JSON line.
//!
//! A record says a run happened without carrying what the person sent or received. Inputs appear as
//! the consent screen shows them (role, kind, size). Outputs appear as kind, media type, size and a
//! SHA-256 of the bytes; returned text as its length and hash. A link is never kept, because a
//! hosted result link can carry a signed token. Error and notice text is kept: the adapters word
//! it for people, and the leak check below covers it.
//!
//! Invariant: [`RunRecord::to_json_line`] refuses to produce a line that contains any of the
//! secrets it is given, so a key that slipped into an error message never reaches the file.

use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use craft_ai_core::consent::DisclosureItem;
use craft_ai_core::{AiError, ErrorKind, Event, JobState, Locality, Payload, Usage};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::client::{Finished, Plan};

/// Format version of a record line. Raise it when a field changes meaning.
pub const RUN_RECORD_VERSION: u32 = 1;

/// One finished run, ready to keep as evidence. Holds no key, no prompt, no result bytes, no link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RunRecord {
    pub version: u32,
    pub app: String,
    pub role: String,
    pub capability: String,
    /// Adapter name as stored on the connection.
    pub provider: String,
    pub connection: String,
    pub model: String,
    pub locality: Locality,
    /// `scheme://host[:port]`, as consent and history record it.
    pub destination: String,
    pub sent: Vec<DisclosureItem>,
    /// Parameter names only, never values.
    pub param_names: Vec<String>,
    /// RFC 3339, UTC, to the millisecond.
    pub started_at: String,
    pub elapsed_ms: u64,
    pub outcome: RunOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RunError>,
    pub outputs: Vec<RunOutput>,
    /// Streamed text, measured and hashed rather than kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streamed_text: Option<Digested>,
    pub notices: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// How the job ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Succeeded,
    Failed,
    Cancelled,
    /// The caller built the [`Finished`] from a job that had not ended (`Queued` or `Running`), for
    /// example from its own polling. [`Job::wait`](crate::Job::wait) never gives this: it returns
    /// an error on a timeout, so a run that times out there leaves no record.
    Unfinished,
}

/// The failure, as the adapter worded it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RunError {
    pub kind: ErrorKind,
    pub message: String,
}

/// Length and SHA-256 (lowercase hex) of some bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Digested {
    pub bytes: u64,
    pub sha256: String,
}

impl Digested {
    /// The length and hash of `data`.
    fn of(data: &[u8]) -> Self {
        Self {
            bytes: data.len() as u64,
            sha256: hex(&Sha256::digest(data)),
        }
    }
}

/// One result the job produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunOutput {
    Text {
        #[serde(flatten)]
        digest: Digested,
    },
    Inline {
        media_type: String,
        #[serde(flatten)]
        digest: Digested,
    },
    /// A link the job returned. Only its type and declared size are kept, never the address.
    Link {
        media_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        size_bytes: Option<u64>,
    },
}

impl RunOutput {
    /// What the record keeps of one result: text and inline bytes are measured and hashed, a
    /// link keeps only its type and declared size.
    fn of(payload: &Payload) -> Self {
        match payload {
            Payload::Text { text } => Self::Text {
                digest: Digested::of(text.as_bytes()),
            },
            Payload::Inline { media_type, data } => Self::Inline {
                media_type: media_type.as_str().to_owned(),
                digest: Digested::of(data),
            },
            Payload::Handle(h) => Self::Link {
                media_type: h.media_type.as_str().to_owned(),
                size_bytes: h.size_bytes,
            },
        }
    }
}

impl RunRecord {
    /// Builds the record of `finished`, the result of running `plan`, which started at `started`
    /// and took `elapsed`.
    pub fn new(
        plan: &Plan,
        finished: &Finished,
        started: DateTime<Utc>,
        elapsed: Duration,
    ) -> Self {
        let disclosure = plan.disclosure();
        let (outcome, error) = match &finished.state {
            JobState::Succeeded => (RunOutcome::Succeeded, None),
            JobState::Failed { error } => (
                RunOutcome::Failed,
                Some(RunError {
                    kind: error.kind,
                    message: error.message.clone(),
                }),
            ),
            JobState::Cancelled => (RunOutcome::Cancelled, None),
            JobState::Queued | JobState::Running { .. } => (RunOutcome::Unfinished, None),
        };
        let mut streamed = String::new();
        let mut outputs = Vec::new();
        let mut notices = Vec::new();
        for event in &finished.events {
            match event {
                Event::TextDelta { text } => streamed.push_str(text),
                Event::Output(o) => outputs.push(RunOutput::of(&o.payload)),
                Event::Notice { message } => notices.push(message.clone()),
                Event::ToolCall { .. } => {}
            }
        }
        Self {
            version: RUN_RECORD_VERSION,
            app: disclosure.app.as_str().to_owned(),
            role: plan.role_id.as_str().to_owned(),
            capability: disclosure.capability.as_str().to_owned(),
            provider: plan.provider().to_owned(),
            connection: disclosure.connection.as_str().to_owned(),
            model: plan.model().as_str().to_owned(),
            locality: disclosure.locality,
            destination: disclosure.destination.clone(),
            sent: disclosure.items.clone(),
            param_names: disclosure.param_names.clone(),
            started_at: started.to_rfc3339_opts(SecondsFormat::Millis, true),
            elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            outcome,
            error,
            outputs,
            streamed_text: (!streamed.is_empty()).then(|| Digested::of(streamed.as_bytes())),
            notices,
            usage: finished.usage.clone(),
        }
    }

    /// The record as one line of JSON (no trailing newline), refused when it contains any of
    /// `secrets`. Empty secrets are ignored, since an empty string is in every line.
    pub fn to_json_line(&self, secrets: &[&str]) -> Result<String, AiError> {
        let line = serde_json::to_string(self).map_err(|e| {
            AiError::new(
                ErrorKind::Internal,
                format!("could not write the run record: {e}"),
            )
        })?;
        if secrets.iter().any(|s| !s.is_empty() && line.contains(s)) {
            return Err(AiError::invalid(
                "the run record contains a key, so it was not written",
            ));
        }
        Ok(line)
    }
}

/// Lowercase hex of `bytes`, two characters per byte.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}
