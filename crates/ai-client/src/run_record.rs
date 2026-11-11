//! Evidence of one real run through [`AiClient`](crate::AiClient): what was asked, where it went,
//! what came back and how long it took, kept as one JSON line.
//!
//! A record says a run happened without carrying what the person sent or received. Inputs appear as
//! the consent screen shows them (role, kind, size). Outputs appear as kind, media type, size and a
//! SHA-256 of the bytes; returned text as its length and hash. A link is never kept, because a
//! hosted result link can carry a signed token. Error and notice text is kept, because the
//! adapters word it for people, but it is cleaned when the record is built: the given secrets, the
//! request's own text inputs, the text values of its parameters (each also in its JSON-escaped
//! form) and anything shaped like a link are replaced.
//!
//! Invariants: the error and notice text a record holds contains none of the secrets given to
//! [`RunRecord::new`]. The other fields are copied from the plan and are not checked there, and the
//! fields are public, so write records with [`RunRecord::to_json_line`]: it refuses a line that
//! contains a secret, in plain or JSON-escaped form.

use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use craft_ai_core::consent::DisclosureItem;
use craft_ai_core::{AiError, ErrorKind, Event, JobState, Locality, Payload, Usage};
use serde::Serialize;
use serde_json::Value;
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
    /// inputs, of the text the run returned and of links before it is kept.
    pub fn new(
        plan: &Plan,
        finished: &Finished,
        started: DateTime<Utc>,
        elapsed: Duration,
        secrets: &[&str],
    ) -> Self {
        let disclosure = plan.disclosure();
        let mut prompts: Vec<&str> = plan
            .request
            .inputs
            .iter()
            .filter_map(|i| match &i.payload {
                Payload::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        prompts.extend(text_values(plan.request.params.values()));
        // What the run returned as text, gathered first so a message quoting it is cleaned too: the
        // whole streamed text (single deltas can be a letter long) and each text result.
        let mut streamed = String::new();
        let mut results: Vec<&str> = Vec::new();
        for event in &finished.events {
            match event {
                Event::TextDelta { text } => streamed.push_str(text),
                Event::Output(o) => {
                    if let Payload::Text { text } = &o.payload {
                        results.push(text);
                    }
                }
                Event::Notice { .. } | Event::ToolCall { .. } => {}
            }
        }
        results.push(&streamed);
        let clean = |text: &str| scrub(text, secrets, &prompts, &results);
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
        let mut outputs = Vec::new();
        let mut notices = Vec::new();
        for event in &finished.events {
            match event {
                Event::Output(o) => outputs.push(RunOutput::of(&o.payload)),
                Event::Notice { message } => notices.push(clean(message)),
                Event::TextDelta { .. } | Event::ToolCall { .. } => {}
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

/// What a removed stretch held, in the order that names a merged stretch: a stretch that held a
/// key reads "[key removed]" whatever else it covered, then a link, then returned text, then
/// request text.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Removed {
    RequestText,
    ResultText,
    Link,
    Key,
}

impl Removed {
    /// The placeholder that replaces the stretch.
    fn placeholder(self) -> &'static str {
        match self {
            Self::RequestText => "[request text removed]",
            Self::ResultText => "[result text removed]",
            Self::Link => "[link removed]",
            Self::Key => "[key removed]",
        }
    }
}

/// `text` with each secret, each request text, each returned text and each link replaced by a
/// placeholder. All of them are found in the original text and overlapping matches are removed together, so a key inside a
/// prompt cannot break the prompt's match, and request text inside a link cannot split the link
/// and leave the rest of its address (and any signed token) behind.
fn scrub(text: &str, secrets: &[&str], prompts: &[&str], results: &[&str]) -> String {
    // (start, end, what it held), byte offsets into `text` on character boundaries.
    let mut spans: Vec<(usize, usize, Removed)> = link_spans(text);
    // Each text as given and, when JSON would write it differently, as JSON escapes it: an adapter
    // that quotes a JSON body back in its message shows a backslash and `n` for a line break.
    let needles: Vec<(String, Removed)> = secrets
        .iter()
        .map(|s| (*s, Removed::Key))
        .chain(prompts.iter().map(|p| (*p, Removed::RequestText)))
        .chain(results.iter().map(|r| (*r, Removed::ResultText)))
        .flat_map(|(s, kind)| {
            let escaped = json_escaped(s);
            let differs = escaped != s;
            [(s.to_owned(), kind)]
                .into_iter()
                .chain(differs.then_some((escaped, kind)))
        })
        .collect();
    for (needle, kind) in needles.iter().filter(|(n, _)| !n.is_empty()) {
        let (needle, kind) = (needle.as_str(), *kind);
        // Every occurrence, overlapping ones included (`match_indices` skips those): the next
        // search starts one character after the last match began.
        let step = needle.chars().next().map_or(1, char::len_utf8);
        let mut from = 0;
        while let Some(found) = text.get(from..).and_then(|rest| rest.find(needle)) {
            let at = from + found;
            spans.push((at, at + needle.len(), kind));
            from = at + step;
        }
    }
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize, Removed)> = Vec::new();
    for (start, end, kind) in spans {
        match merged.last_mut() {
            Some(last) if start < last.1 => {
                last.1 = last.1.max(end);
                last.2 = last.2.max(kind);
            }
            _ => merged.push((start, end, kind)),
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    for (start, end, kind) in merged {
        out.push_str(text.get(from..start).unwrap_or_default());
        out.push_str(kind.placeholder());
        from = end;
    }
    out.push_str(text.get(from..).unwrap_or_default());
    out
}

/// Every text value inside `values`, however deeply nested in arrays and objects. Walked with a
/// list rather than recursion, so a deeply nested parameter cannot exhaust the stack.
fn text_values<'a>(values: impl Iterator<Item = &'a Value>) -> Vec<&'a str> {
    let mut pending: Vec<&Value> = values.collect();
    let mut found = Vec::new();
    while let Some(value) = pending.pop() {
        match value {
            Value::String(s) => found.push(s.as_str()),
            Value::Array(items) => pending.extend(items),
            Value::Object(map) => pending.extend(map.values()),
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }
    found
}

/// Where each word that looks like an address (`scheme://...`, the same written as JSON escapes it
/// with each slash after a backslash, or `file:...`) sits in `text`, so a signed result link in an
/// adapter's message is not kept. Words are split on whitespace, and the whitespace itself is never
/// part of a span, so spacing is left as it was.
fn link_spans(text: &str) -> Vec<(usize, usize, Removed)> {
    let mut spans = Vec::new();
    let mut at = 0;
    for piece in text.split_inclusive(char::is_whitespace) {
        let word = piece.trim_end_matches(char::is_whitespace);
        if word.contains("://") || word.contains(r":\/\/") || word.contains("file:") {
            spans.push((at, at + word.len(), Removed::Link));
        }
        at += piece.len();
    }
    spans
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
