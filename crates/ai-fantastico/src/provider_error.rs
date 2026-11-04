//! Failures reported by the model's provider, kept in structured form.
//!
//! Every decision (retry, mint a new key, give up) is taken on the HTTP status and on the
//! provider's `metadata.error_type`, **never on the message text**: moderation errors echo the
//! user's prompt back, and matching on prose would pick up whatever the user wrote.

use serde_json::Value;

use crate::errors::Error;
use crate::time::{now_ms, parse_http_date_ms};
use crate::transport::HttpResponse;

/// The `error_type` values the provider documents. Anything else (a new name, or `unmapped`)
/// decides nothing, and the numeric status is used instead.
const CONCLUSIVE: &[&str] = &[
    "context_length_exceeded",
    "max_tokens_exceeded",
    "token_limit_exceeded",
    "string_too_long",
    "authentication",
    "permission_denied",
    "payment_required",
    "rate_limit_exceeded",
    "provider_overloaded",
    "provider_unavailable",
    "invalid_request",
    "invalid_prompt",
    "not_found",
    "precondition_failed",
    "payload_too_large",
    "unprocessable",
    "content_policy_violation",
    "refusal",
    "invalid_image",
    "image_too_large",
    "image_too_small",
    "unsupported_image_format",
    "image_not_found",
    "image_download_failed",
    "server",
    "timeout",
];
/// The only failures a new runtime key can fix: a key that is no longer valid, or a cap reached.
const REMINTABLE: &[&str] = &["authentication", "payment_required"];
/// Failures where the provider certainly generated nothing, so the same request can be sent again.
/// Timeouts and unavailable upstreams are left out: the answer may have been generated.
const TRANSIENT: &[&str] = &["rate_limit_exceeded", "provider_overloaded"];
/// A non-JSON error body is cut to this many characters in the message.
const MAX_RAW_MESSAGE: usize = 300;
/// The longest wait a `Retry-After` is taken at.
const MAX_RETRY_AFTER_MS: f64 = 86_400_000.0;

/// A failed provider call: status, classification and the provider's own message.
#[derive(Clone, Debug)]
pub(crate) struct ProviderError {
    pub message: String,
    pub http_status: u16,
    pub provider_code: Option<u16>,
    pub error_type: Option<String>,
    pub retry_after_ms: Option<u64>,
}

/// The provider's error body: `{"error": {"code", "message", "metadata": {"error_type"}}}`.
struct Envelope {
    message: Option<String>,
    code: Option<u16>,
    error_type: Option<String>,
}

fn read_envelope(payload: &Value) -> Option<Envelope> {
    let err = payload.get("error")?.as_object()?;
    Some(Envelope {
        message: err
            .get("message")
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty())
            .map(str::to_owned),
        code: err
            .get("code")
            .and_then(Value::as_u64)
            .and_then(|c| u16::try_from(c).ok()),
        error_type: err
            .get("metadata")
            .and_then(|m| m.get("error_type"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn truncate(text: &str) -> String {
    match text.char_indices().nth(MAX_RAW_MESSAGE) {
        Some((at, _)) => format!("{}…", text.get(..at).unwrap_or(text)),
        None => text.to_owned(),
    }
}

/// The wait a `Retry-After` header asks for, in milliseconds. Both standard forms are read
/// (seconds, or an HTTP date); a nonsensical value counts as no value.
pub(crate) fn retry_after_ms(header: Option<&str>) -> Option<u64> {
    let h = header?.trim();
    if let Ok(seconds) = h.parse::<f64>() {
        return (seconds.is_finite() && seconds > 0.0)
            .then(|| (seconds * 1000.0).min(MAX_RETRY_AFTER_MS).round() as u64);
    }
    let delta = parse_http_date_ms(h)?.saturating_sub(now_ms());
    u64::try_from(delta).ok().filter(|d| *d > 0)
}

impl ProviderError {
    /// The failure described by a non-`2xx` response.
    pub fn from_response(res: &HttpResponse) -> Self {
        let parsed = res.json().as_ref().and_then(read_envelope);
        let detail = parsed
            .as_ref()
            .and_then(|p| p.message.clone())
            .unwrap_or_else(|| truncate(&res.body));
        ProviderError {
            message: format!("the model's provider answered {}: {detail}", res.status),
            http_status: res.status,
            provider_code: parsed.as_ref().and_then(|p| p.code),
            error_type: parsed.and_then(|p| p.error_type),
            retry_after_ms: retry_after_ms(res.header("Retry-After")),
        }
    }

    /// The failure delivered inside a `200 OK` body, which the provider documents as possible. The
    /// status that matters is then `error.code`, not the transport's `200`.
    pub fn in_body(payload: &Value) -> Option<Self> {
        let p = read_envelope(payload)?;
        Some(ProviderError {
            message: format!(
                "the model's provider answered {}: {}",
                p.code.unwrap_or(200),
                p.message.as_deref().unwrap_or("an error in the reply")
            ),
            http_status: 200,
            provider_code: p.code,
            error_type: p.error_type,
            retry_after_ms: None,
        })
    }

    fn conclusive_type(&self) -> Option<&str> {
        self.error_type
            .as_deref()
            .filter(|t| CONCLUSIVE.contains(t))
    }

    fn code(&self) -> u16 {
        self.provider_code.unwrap_or(self.http_status)
    }

    /// Whether a new runtime key may fix this: only `401` (key no longer valid) and `402` (cap
    /// reached). A `402` doesn't say whether the key's cap or the account ran out, so a new key is
    /// tried rather than guessed about.
    pub fn is_remintable(&self) -> bool {
        match self.conclusive_type() {
            Some(t) => REMINTABLE.contains(&t),
            None => matches!(self.code(), 401 | 402),
        }
    }

    /// Whether the same request may succeed a moment later. Never true together with
    /// [`Self::is_remintable`], so a retry never chains into a new key.
    pub fn is_transient(&self) -> bool {
        match self.conclusive_type() {
            Some(t) => TRANSIENT.contains(&t),
            None => matches!(self.code(), 429 | 503),
        }
    }

    /// The error when the provider classified the failure: `PROVIDER_<TYPE>` with the provider's
    /// status.
    pub fn named(&self) -> Option<Error> {
        let t = self.conclusive_type()?;
        let status = self.provider_code.unwrap_or(if self.http_status == 200 {
            502
        } else {
            self.http_status
        });
        Some(Error::new(
            self.message.clone(),
            &format!("PROVIDER_{}", t.to_ascii_uppercase()),
            status,
        ))
    }

    /// [`Self::named`], or `INTERNAL_ERROR` when unclassified.
    pub fn into_error(self) -> Error {
        self.named()
            .unwrap_or_else(|| Error::new(self.message, "INTERNAL_ERROR", 500))
    }
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect(),
        _ => String::new(),
    }
}

/// The `200 OK` that is not a success: the output budget ran out and nothing usable came back.
///
/// Exactly one combination is caught (`finish_reason: "length"`, no tool calls, no text): a false
/// positive here would fail an answer the caller could have used.
pub(crate) fn empty_completion(
    payload: &Value,
    model: &str,
    max_tokens: Option<u64>,
) -> Option<Error> {
    let choice = payload.pointer("/choices/0")?;
    if choice.get("finish_reason").and_then(Value::as_str) != Some("length") {
        return None;
    }
    let message = choice.get("message").unwrap_or(&Value::Null);
    if message
        .get("tool_calls")
        .and_then(Value::as_array)
        .is_some_and(|c| !c.is_empty())
    {
        return None;
    }
    if !text_of(message.get("content").unwrap_or(&Value::Null))
        .trim()
        .is_empty()
    {
        return None;
    }
    let used = payload
        .pointer("/usage/completion_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let reasoning = payload
        .pointer("/usage/completion_tokens_details/reasoning_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let budget = max_tokens.map_or_else(
        || "its output budget".to_owned(),
        |n| format!("{n} output tokens"),
    );
    let (cause, remedy) = if reasoning > 0 {
        (
            format!("spent {reasoning} on reasoning and returned no content"),
            "turn reasoning off or raise `max_tokens`",
        )
    } else {
        (
            "ran out of budget without returning content".to_owned(),
            "raise `max_tokens`",
        )
    };
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(model);
    Some(Error::new(
        format!("{model} reached {budget} ({used} used), {cause}: {remedy}"),
        "EMPTY_COMPLETION",
        502,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn res(status: u16, error_type: Option<&str>) -> HttpResponse {
        let metadata = error_type.map_or(Value::Null, |t| json!({"error_type": t}));
        HttpResponse {
            status,
            headers: vec![],
            body: json!({"error": {"code": status, "message": "no", "metadata": metadata}})
                .to_string(),
        }
    }

    #[test]
    fn transient_and_remintable_are_disjoint_and_decided_on_status_and_type() {
        for status in [
            400u16, 401, 402, 403, 404, 408, 413, 429, 500, 502, 503, 504,
        ] {
            for t in [
                None,
                Some("authentication"),
                Some("payment_required"),
                Some("rate_limit_exceeded"),
                Some("provider_overloaded"),
                Some("unmapped"),
                Some("timeout"),
            ] {
                let p = ProviderError::from_response(&res(status, t));
                assert!(!(p.is_transient() && p.is_remintable()), "{status} {t:?}");
            }
        }
        let p = |s, t| ProviderError::from_response(&res(s, t));
        assert!(p(401, None).is_remintable() && p(402, None).is_remintable());
        assert!(p(429, None).is_transient() && p(503, None).is_transient());
        assert!(!p(502, None).is_transient() && !p(504, None).is_transient());
        // A documented type wins over the status; an unknown one decides nothing.
        assert!(p(500, Some("payment_required")).is_remintable());
        assert!(!p(401, Some("content_policy_violation")).is_remintable());
        assert!(p(429, Some("unmapped")).is_transient());
        assert!(!p(429, Some("timeout")).is_transient());
    }

    #[test]
    fn a_named_failure_keeps_the_providers_type_and_status() {
        let e =
            ProviderError::from_response(&res(403, Some("content_policy_violation"))).into_error();
        assert_eq!(
            (e.code.as_str(), e.status),
            ("PROVIDER_CONTENT_POLICY_VIOLATION", 403)
        );
        let e = ProviderError::from_response(&res(500, None)).into_error();
        assert_eq!((e.code.as_str(), e.status), ("INTERNAL_ERROR", 500));
        // Inside a `200`, the status is the body's code, or 502 when it has none.
        let inside =
            ProviderError::in_body(&json!({"error": {"metadata": {"error_type": "server"}}}))
                .unwrap()
                .into_error();
        assert_eq!(
            (inside.code.as_str(), inside.status),
            ("PROVIDER_SERVER", 502)
        );
        assert!(ProviderError::in_body(&json!({"error": "a string"})).is_none());
        assert!(ProviderError::in_body(&json!({"choices": []})).is_none());
    }

    #[test]
    fn an_unreadable_body_is_cut_short_in_the_message() {
        let r = HttpResponse {
            status: 502,
            headers: vec![],
            body: "é".repeat(1000),
        };
        let p = ProviderError::from_response(&r);
        assert!(p.message.chars().count() < 400, "{}", p.message);
        assert!(p.message.ends_with('…'));
    }

    #[test]
    fn retry_after_reads_seconds_and_dates_and_ignores_nonsense() {
        assert_eq!(retry_after_ms(Some("1.5")), Some(1500));
        assert_eq!(retry_after_ms(Some(" 2 ")), Some(2000));
        assert_eq!(retry_after_ms(Some("1e12")), Some(86_400_000));
        // A date in the past asks for no wait.
        assert_eq!(retry_after_ms(Some("Thu, 01 Jan 1970 00:00:10 GMT")), None);
        let soon = retry_after_ms(Some("Fri, 01 Jan 9999 00:00:00 GMT"));
        assert!(soon.is_some_and(|ms| ms > 0));
        for bad in ["0", "-1", "NaN", "inf", "abc", ""] {
            assert_eq!(retry_after_ms(Some(bad)), None, "{bad}");
        }
        assert_eq!(retry_after_ms(None), None);
    }

    #[test]
    fn only_an_empty_reply_at_the_limit_is_an_empty_completion() {
        let at_limit = |message: Value, usage: Value| json!({"choices": [{"message": message, "finish_reason": "length"}], "usage": usage});
        let e = empty_completion(
            &at_limit(json!({"content": ""}), json!({"completion_tokens": 64})),
            "m",
            Some(64),
        )
        .unwrap();
        assert_eq!(e.code, "EMPTY_COMPLETION");
        assert!(
            e.message.contains("64 output tokens (64 used)"),
            "{}",
            e.message
        );
        let reasoning = empty_completion(
            &at_limit(
                json!({"content": [{"type": "text", "text": " "}]}),
                json!({"completion_tokens": 9, "completion_tokens_details": {"reasoning_tokens": 9}}),
            ),
            "m",
            None,
        )
        .unwrap();
        assert!(
            reasoning.message.contains("reasoning"),
            "{}",
            reasoning.message
        );
        for usable in [
            at_limit(json!({"content": "half an answer"}), json!({})),
            at_limit(
                json!({"content": null, "tool_calls": [{"id": "t"}]}),
                json!({}),
            ),
            json!({"choices": [{"message": {"content": ""}, "finish_reason": "stop"}]}),
            json!({"choices": []}),
        ] {
            assert!(empty_completion(&usable, "m", None).is_none(), "{usable}");
        }
    }
}
