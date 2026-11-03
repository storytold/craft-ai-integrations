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
use craft_ai_core::{
    ConsentLedger, InputRole, Payload, Request, Role,
    job::{Event, Output},
};
use craft_ai_features::Feature;
use serde_json::Value;

/// The outcome of routing one MCP tool call.
#[derive(Debug, Clone)]
pub enum ToolCallResult {
    /// The job ran to completion. `text` holds human-readable output lines (text results and a
    /// summary of any media the provider returned).
    Succeeded { text: Vec<String> },
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
    let request = match build_request(&feature, &args) {
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
        Ok(finished) => ToolCallResult::Succeeded {
            text: extract_text(&finished.events),
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
        return decode_inline(b64, "image/png");
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
        return decode_inline(b64, "image/png");
    }
    Err(format!(
        "'{feature_id}' needs a mask (pass `mask_path` or `mask_base64`)"
    ))
}

fn read_inline(path: &str) -> Result<Payload, String> {
    let data = std::fs::read(path).map_err(|e| format!("cannot read source file '{path}': {e}"))?;
    Payload::inline("image/png", data).map_err(|e| e.to_string())
}

fn decode_inline(b64: &str, media_type: &str) -> Result<Payload, String> {
    let data = base64_decode(b64).ok_or_else(|| format!("bad base64 for source"))?;
    Payload::inline(media_type, data).map_err(|e| e.to_string())
}

/// Minimal standard-base64 decode (no external dependency). Returns `None` on bad input.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let alphabet = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::new();
    let mut buf: [u8; 4] = [0; 4];
    let mut n = 0usize;
    for ch in input.chars() {
        if ch == '=' || ch.is_whitespace() {
            continue;
        }
        match alphabet(ch as u8) {
            Some(v) => {
                buf[n] = v;
                n += 1;
                if n == 4 {
                    out.push((buf[0] << 2) | (buf[1] >> 4));
                    out.push(((buf[1] & 0xF) << 4) | (buf[2] >> 2));
                    out.push(((buf[2] & 3) << 6) | buf[3]);
                    n = 0;
                }
            }
            None => return None,
        }
    }
    if n == 2 {
        out.push((buf[0] << 2) | (buf[1] >> 4));
    } else if n == 3 {
        out.push((buf[0] << 2) | (buf[1] >> 4));
        out.push(((buf[1] & 0xF) << 4) | (buf[2] >> 2));
    }
    Some(out)
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

/// A small helper so callers can build a `Role` from a catalogue id without re-reading the list.
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
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.clone())
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
        let b64 = base64_encode_inline(b"\x89PNG\x0d\x0a\x1a\x0a");
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
        let enc = base64_encode_inline(&data);
        assert_eq!(base64_decode(&enc), Some(data));
    }

    #[test]
    fn role_for_returns_the_catalogue_role() {
        assert!(role_for("select-subject").is_some());
        assert!(role_for("not-a-tool").is_none());
    }

    /// Local helper mirroring `payload::base64_encode` so tests can build inputs without a dep.
    fn base64_encode_inline(data: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(data.len().div_ceil(3).saturating_mul(4));
        for chunk in data.chunks(3) {
            let b0 = u32::from(chunk.first().copied().unwrap_or(0));
            let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
            let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
            let n = (b0 << 16) | (b1 << 8) | b2;
            for shift in [18u32, 12, 6, 0].into_iter().take(chunk.len() + 1) {
                let idx = ((n >> shift) & 63) as usize;
                out.push(ALPHABET.get(idx).copied().map_or('?', char::from));
            }
            for _ in chunk.len()..3 {
                out.push('=');
            }
        }
        out
    }
}
