//! Evidence of one real run through [`AiClient`](crate::AiClient): what was asked, where it went,
//! what came back and how long it took, kept as one JSON line.
//!
//! A record says a run happened without carrying what the person sent or received. Inputs appear as
//! the consent screen shows them (role, kind, size). Outputs appear as kind, media type, size and a
//! SHA-256 of the bytes; returned text as its length and hash. A link is never kept, because a
//! hosted result link can carry a signed token. Error and notice text is kept, because the
//! adapters word it for people, but it is cleaned when the record is built: the given secrets, the
//! request's own text inputs and anything shaped like a link are replaced.
//!
//! Invariants: a record holds none of the secrets given to [`RunRecord::new`], so serialising it by
//! any route is safe; and [`RunRecord::to_json_line`] still refuses a line that contains a secret,
//! in plain or JSON-escaped form, as a second guard.

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
    /// and took `elapsed`. Error and notice text is cleaned of `secrets`, of the request's text
    /// inputs and of links before it is kept.
    pub fn new(
        plan: &Plan,
        finished: &Finished,
        started: DateTime<Utc>,
        elapsed: Duration,
        secrets: &[&str],
    ) -> Self {
        let disclosure = plan.disclosure();
        let prompts: Vec<&str> = plan
            .request
            .inputs
            .iter()
            .filter_map(|i| match &i.payload {
                Payload::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let clean = |text: &str| scrub(text, secrets, &prompts);
        let (outcome, error) = match &finished.state {
            JobState::Succeeded => (RunOutcome::Succeeded, None),
            JobState::Failed { error } => (
                RunOutcome::Failed,
                Some(RunError {
                    kind: error.kind,
                    message: clean(&error.message),
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
                Event::Notice { message } => notices.push(clean(message)),
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
        if secrets
            .iter()
            .any(|s| !s.is_empty() && (line.contains(s) || line.contains(&json_escaped(s))))
        {
            return Err(AiError::invalid(
                "the run record contains a key, so it was not written",
            ));
        }
        Ok(line)
    }
}

/// `text` with each secret, each request text and each link replaced by a placeholder. Secrets go
/// first, so a key inside a link or a prompt is gone whatever else matches.
fn scrub(text: &str, secrets: &[&str], prompts: &[&str]) -> String {
    let mut out = text.to_owned();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        out = out.replace(secret, "[key removed]");
    }
    for prompt in prompts.iter().filter(|p| !p.is_empty()) {
        out = out.replace(prompt, "[request text removed]");
    }
    without_links(&out)
}

/// `text` with every word that looks like an address (`scheme://...` or `file:...`) replaced, so a
/// signed result link in an adapter's message is not kept. Spacing is left as it was.
fn without_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for piece in text.split_inclusive(char::is_whitespace) {
        let word = piece.trim_end_matches(char::is_whitespace);
        if word.contains("://") || word.contains("file:") {
            out.push_str("[link removed]");
        } else {
            out.push_str(word);
        }
        out.push_str(&piece[word.len()..]);
    }
    out
}

/// `secret` as JSON writes it inside a string, so a secret holding a quote, backslash or control
/// character is still found in a serialised line.
fn json_escaped(secret: &str) -> String {
    let quoted = serde_json::to_string(secret).unwrap_or_default();
    quoted
        .strip_prefix('"')
        .and_then(|q| q.strip_suffix('"'))
        .unwrap_or(&quoted)
        .to_owned()
}

/// Lowercase hex of `bytes`, two characters per byte.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}
