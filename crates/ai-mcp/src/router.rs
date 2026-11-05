//! Routes one MCP tool call through the Craft AI client.
//!
//! This is the heart of the bridge: an MCP `tools/call` becomes a Craft AI job. The flow mirrors
//! what every app does (see `craft_ai_client::AiClient` docs): check [`status`](AiClient::status)
//! so a missing model or key surfaces as a plain, actionable reason; build a request from the
//! caller's arguments; plan it (what would leave the machine); honor consent; then start and
//! wait on the job. Nothing here talks to a real provider — that is the adapter's job, reached
//! through [`AiClient`].

use std::time::Duration;

use craft_ai_client::{AiClient, RoleStatus};
use craft_ai_media::sniff;
use craft_ai_core::{
    ConsentLedger, InputRole, JobState, Payload, PayloadKind, Request, Role,
    job::{Event, Output},
};
use craft_ai_features::Feature;
use serde_json::Value;

/// An inline media asset returned by a job (bytes + MIME type), surfaced as MCP image content.
#[derive(Debug, Clone)]
pub struct Media {
    pub mime_type: String,
    pub data: Vec<u8>,
}

/// The outcome of routing one MCP tool call.
#[derive(Debug, Clone)]
pub enum ToolCallResult {
    /// The job ran to completion. `text` holds human-readable output lines; `media` holds any
    /// inline image assets (bytes + MIME type) returned by the provider.
    Succeeded { text: Vec<String>, media: Vec<Media> },
    /// The tool is not usable yet. `reason` is safe to show verbatim and points at what is missing
    /// (no model chosen, key absent, file gone) — this is the "prompt to install" signal.
    Unavailable { reason: String },
    /// Sending data needs the user's agreement first. `disclosure` names exactly what would leave
    /// the machine (kinds and sizes, never contents).
    NeedsConsent { disclosure: String },
    /// A hard failure: unknown tool, bad arguments, a plan the model rejects, or the job erroring
    /// out / timing out.
    Failed { message: String },
}

impl ToolCallResult {
    /// True only for a successful tool call (a `Succeeded` result).
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Succeeded { .. })
    }
}

/// Routes one MCP tool call through `client`.
///
/// - `feature_id` must be a built-in catalogue id (e.g. `generate-image`).
/// - `args` is the JSON object the client passed as tool arguments; it is mapped onto the feature's
///   declared inputs (`prompt`, `source_path`/`source_base64`, `mask_path`/`mask_base64`).
/// - When `auto_consent` is true, any required agreement is granted before starting (for local
///   development / trusted hosts). When false and a remote destination needs consent, the call
///   returns [`ToolCallResult::NeedsConsent`] instead of sending anything.
pub fn route_tool(
    client: &AiClient,
    feature_id: &str,
    args: Value,
    auto_consent: bool,
    timeout: Duration,
) -> ToolCallResult {
    let features = craft_ai_features::builtin();
    let Some(feature) = features.iter().find(|f| f.id == feature_id) else {
        return ToolCallResult::Failed {
            message: format!("unknown tool '{feature_id}'"),
        };
    };
    let role = match feature.role() {
        Some(r) => r,
        None => {
            return ToolCallResult::Failed {
                message: "tool has no role".into(),
            };
        }
    };

    // 1. Status — the plain-language "what is missing" check.
    match client.status(&role) {
        RoleStatus::Unavailable { reason } => return ToolCallResult::Unavailable { reason },
        RoleStatus::Ready(_) => {}
    }

    // 2. Build a request from the caller's arguments.
    let request = match build_request(feature, &args) {
        Ok(r) => r,
        Err(e) => return ToolCallResult::Failed { message: e },
    };

    // 3. Plan — resolve the model and check the request against it. No key is read yet.
    let plan = match client.plan(&role, request) {
        Ok(p) => p,
        Err(e) => {
            return ToolCallResult::Failed {
                message: e.to_string(),
            };
        }
    };

    // 4. Consent — local connections never ask; remote ones need an agreement (or auto-grant).
    let mut ledger = load_ledger(client);
    if plan.needs_consent(&ledger) {
        if auto_consent {
            ledger.grant(plan.disclosure());
        } else {
            return ToolCallResult::NeedsConsent {
                disclosure: serde_json::to_string(&plan.disclosure()).unwrap_or_default(),
            };
        }
    }

    // 5. Start and wait on the job.
    let job = match client.start(&plan, &ledger) {
        Ok(j) => j,
        Err(e) => {
            return ToolCallResult::Failed {
                message: e.to_string(),
            };
        }
    };
    match job.wait(Duration::from_millis(100), timeout) {
        Ok(finished) => match finished.state {
            JobState::Succeeded => ToolCallResult::Succeeded {
                text: extract_text(&finished.events),
                media: extract_media(&finished.events),
            },
            JobState::Failed { error } => ToolCallResult::Failed {
                message: error.message,
            },
            JobState::Cancelled => ToolCallResult::Failed {
                message: "the job was cancelled".into(),
            },
            _ => ToolCallResult::Failed {
                message: "the job did not finish in a supported state".into(),
            },
        },
        Err(e) => ToolCallResult::Failed {
            message: e.to_string(),
        },
    }
}

/// Loads the app's saved consent ledger, or an empty one when there is none. An empty ledger means
/// every remote destination asks for agreement (or auto-grants).
fn load_ledger(client: &AiClient) -> ConsentLedger {
    match client.consent_file() {
        Some(file) => file.load_or_empty(),
        None => ConsentLedger::default(),
    }
}

/// Maps the caller's JSON arguments onto the feature's declared inputs. Public so it can be tested
/// in isolation and reused if a tool gains new argument conventions.
pub fn build_request(feature: &Feature, args: &Value) -> Result<Request, String> {
    let mut request = Request::new(feature.capability.clone());
    for input in &feature.inputs {
        match input {
            InputRole::Prompt => {
                let text = args
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .or_else(|| {
                        args.get("description")
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_default();
                if text.is_empty() {
                    return Err(format!("'{}' needs a prompt (pass `prompt`)", feature.id));
                }
                request = request.with_input(InputRole::Prompt, Payload::text(text));
            }
            InputRole::Source => {
                let payload = source_payload(args, &feature.id)?;
                request = request.with_input(InputRole::Source, payload);
            }
            InputRole::Mask => {
                let payload = mask_payload(args, &feature.id)?;
                request = request.with_input(InputRole::Mask, payload);
            }
            _ => {
                // Clicks / references are app-specific (e.g. a selection tool's clicks). Not yet
                // expressible through plain JSON args in v1.
                return Err(format!(
                    "'{}' needs an unsupported input ({}); send it from the app for now",
                    feature.id, input
                ));
            }
        }
    }
    Ok(request)
}

fn source_payload(args: &Value, feature_id: &str) -> Result<Payload, String> {
    if let Some(path) = args.get("source_path").and_then(|v| v.as_str()) {
        return read_inline(path);
    }
    if let Some(b64) = args.get("source_base64").and_then(|v| v.as_str()) {
        return decode_inline(b64);
    }
    Err(format!(
        "'{feature_id}' needs a source image (pass `source_path` or `source_base64`)"
    ))
}

fn mask_payload(args: &Value, feature_id: &str) -> Result<Payload, String> {
    if let Some(path) = args.get("mask_path").and_then(|v| v.as_str()) {
        return read_inline(path);
    }
    if let Some(b64) = args.get("mask_base64").and_then(|v| v.as_str()) {
        return decode_inline(b64);
    }
    Err(format!(
        "'{feature_id}' needs a mask (pass `mask_path` or `mask_base64`)"
    ))
}

/// The media type of `data`, sniffed from its first bytes rather than trusted from a label. Falls
/// back to PNG when the bytes are unrecognised, so legacy callers that only ever sent PNGs keep
/// working while real JPEG/GIF/etc. inputs get their true type.
fn sniff_type(data: &[u8]) -> &'static str {
    let head = data.get(..sniff::HEAD_LEN).unwrap_or(data);
    sniff::media_type(head)
        .map(|(m, _)| m)
        .unwrap_or("image/png")
}

fn read_inline(path: &str) -> Result<Payload, String> {
    // Reject oversized files before allocating their full contents — a client-supplied path must
    // not let us stream an unbounded blob into memory.
    let meta = std::fs::metadata(path)
        .map_err(|e| format!("cannot stat source file '{path}': {e}"))?;
    if !meta.is_file() {
        return Err(format!("source file '{path}' is not a regular file"));
    }
    let size = meta.len();
    if size > craft_ai_core::payload::DEFAULT_MAX_INLINE_BYTES {
        return Err(format!(
            "source file '{path}' is {} bytes, larger than the {} byte inline limit",
            size,
            craft_ai_core::payload::DEFAULT_MAX_INLINE_BYTES,
        ));
    }
    let data = std::fs::read(path).map_err(|e| format!("cannot read source file '{path}': {e}"))?;
    Payload::inline(sniff_type(&data), data).map_err(|e| e.to_string())
}

fn decode_inline(b64: &str) -> Result<Payload, String> {
    // Strip whitespace (a client may wrap the base64 across lines), then use the strict core
    // decoder so malformed input is rejected rather than silently mangled.
    let compact = b64.chars().filter(|c| !c.is_ascii_whitespace()).collect::<String>();
    let data = craft_ai_core::payload::base64_decode(&compact)
        .ok_or_else(|| "bad base64 for source".to_string())?;
    Payload::inline(sniff_type(&data), data).map_err(|e| e.to_string())
}

/// Human-readable lines from a finished job: text outputs verbatim, media outputs summarised.
fn extract_text(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Output(Output { payload, .. }) => Some(match payload {
                Payload::Text { text } => text.clone(),
                Payload::Inline { media_type, data } => {
                    format!("media ({media_type}, {} bytes)", data.len())
                }
                Payload::Handle(h) => {
                    format!("media handle ({}: {})", h.media_type.as_str(), h.uri)
                }
            }),
            _ => None,
        })
        .collect()
}

/// Inline image assets from a finished job, in order. Only `Payload::Inline` payloads whose sniffed
/// kind is an image are collected; text and handle outputs are left to [`extract_text`].
fn extract_media(events: &[Event]) -> Vec<Media> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Output(Output { payload, .. }) => match payload {
                Payload::Inline { media_type, data } if media_type.kind() == PayloadKind::Image => {
                    Some(Media {
                        mime_type: media_type.as_str().to_string(),
                        data: data.clone(),
                    })
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// A small helper so callers can build a [`Role`] from a catalogue id without re-reading the list.
pub fn role_for(feature_id: &str) -> Option<Role> {
    craft_ai_features::builtin()
        .iter()
        .find(|f| f.id == feature_id)
        .and_then(|f| f.role())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feature(id: &str) -> Feature {
        craft_ai_features::builtin()
            .into_iter()
            .find(|f| f.id == id)
            .unwrap()
    }

    /// An empty JSON object, for "no arguments" cases.
    fn empty_args() -> Value {
        Value::Object(serde_json::Map::new())
    }

    #[test]
    fn build_request_maps_prompt_for_generate_image() {
        let req = build_request(
            &feature("generate-image"),
            &serde_json::json!({"prompt": "a cat"}),
        )
        .unwrap();
        assert_eq!(req.capability.as_str(), "image.generate");
        assert_eq!(req.count(&InputRole::Prompt), 1);
    }

    #[test]
    fn build_request_rejects_a_missing_prompt() {
        let err = build_request(&feature("generate-image"), &empty_args()).unwrap_err();
        assert!(err.contains("prompt"));
    }

    #[test]
    fn build_request_maps_source_and_mask_for_inpaint() {
        // A tiny PNG-ish blob; the media type is what matters for the mapping test.
        let b64 = craft_ai_core::payload::base64_encode(b"\x89PNG\x0d\x0a\x1a\x0a");
        let mut m = serde_json::Map::new();
        m.insert("source_base64".into(), Value::String(b64.clone()));
        m.insert("mask_base64".into(), Value::String(b64));
        m.insert("prompt".into(), Value::String("a cat".to_string()));
        let args = Value::Object(m);
        let req = build_request(&feature("generative-fill"), &args).unwrap();
        assert_eq!(req.count(&InputRole::Source), 1);
        assert_eq!(req.count(&InputRole::Mask), 1);
        assert_eq!(req.count(&InputRole::Prompt), 1);
    }

    #[test]
    fn build_request_rejects_a_missing_source() {
        let err = build_request(&feature("remove-background"), &empty_args()).unwrap_err();
        assert!(err.contains("source"));
    }

    #[test]
    fn base64_roundtrips() {
        let data: Vec<u8> = (0..16).map(|i| i as u8).collect();
        let enc = craft_ai_core::payload::base64_encode(&data);
        assert_eq!(craft_ai_core::payload::base64_decode(&enc), Some(data));
    }

    #[test]
    fn role_for_returns_the_catalogue_role() {
        assert!(role_for("select-subject").is_some());
        assert!(role_for("not-a-tool").is_none());
    }

}
