//! Reading a server-sent-events stream one line at a time.
//!
//! Everything here is pure (no I/O): jobs.rs owns the connection and feeds lines in. The reply
//! is bounded as it grows. No line longer than [`MAX_SSE_LINE`] is accepted, and the total
//! kept - every text delta plus every tool call's name and arguments - may not pass
//! [`MAX_STREAM_TOTAL`], the same size as the whole-reply cap in `http::MAX_BODY`. A server
//! that streams past either is refused rather than allowed to grow the reply until the
//! request timeout.

use craft_ai_core::{AiError, ErrorKind, Usage};
use serde_json::Value;

use crate::chat::{Completion, MAX_TOOL_CALLS, error_text, usage_from};

/// Largest single SSE line accepted, in bytes.
pub(crate) const MAX_SSE_LINE: usize = 1024 * 1024;
/// Largest total a stream may keep: every text delta plus every tool call's name and
/// arguments. The same size as the whole-reply cap (`http::MAX_BODY`), so a stream can never
/// hold more than a whole reply would; a server that streams past it is refused rather than
/// allowed to grow the reply until the request timeout.
pub(crate) const MAX_STREAM_TOTAL: usize = crate::http::MAX_BODY;

/// Reads a server-sent-events stream one line at a time.
#[derive(Debug)]
pub(crate) struct SseState {
    /// All text so far (needed for the structured check at the end).
    pub text: String,
    tools: Vec<(String, String)>,
    pub usage: Option<Usage>,
    /// `[DONE]` was seen.
    pub done: bool,
    /// A `finish_reason` was seen.
    pub finished: bool,
    /// Largest total the stream may keep before it is refused.
    cap: usize,
}

impl Default for SseState {
    /// The full-size cap; only a test shrinks it.
    fn default() -> Self {
        Self {
            text: String::new(),
            tools: Vec::new(),
            usage: None,
            done: false,
            finished: false,
            cap: MAX_STREAM_TOTAL,
        }
    }
}

impl SseState {
    /// Shrinks the total cap so a test can drive a stream past it cheaply.
    #[cfg(test)]
    pub(crate) fn with_cap(mut self, cap: usize) -> Self {
        self.cap = cap;
        self
    }

    /// Bytes kept from the stream so far: the text plus every tool call's name and arguments.
    fn kept(&self) -> usize {
        self.text.len()
            + self
                .tools
                .iter()
                .map(|(name, args)| name.len() + args.len())
                .sum::<usize>()
    }

    /// Feeds one line. Returns the new text, if the line carried any.
    pub fn line(&mut self, raw: &str) -> Result<Option<String>, AiError> {
        let line = raw.trim_end_matches(['\r', '\n']);
        // Blank lines end an event; comments start with a colon; other fields are not used.
        let Some(data) = line.strip_prefix("data:") else {
            return Ok(None);
        };
        let data = data.trim_start();
        if data == "[DONE]" {
            self.done = true;
            return Ok(None);
        }
        let v: Value = serde_json::from_str(data).map_err(|_| {
            AiError::new(ErrorKind::Provider, "the stream held an unreadable event")
        })?;
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
            return Err(AiError::new(ErrorKind::Provider, error_text(e)));
        }
        if let Some(u) = usage_from(&v) {
            self.usage = Some(u);
        }
        let Some(choice) = v.pointer("/choices/0") else {
            return Ok(None);
        };
        if choice.get("finish_reason").is_some_and(|f| !f.is_null()) {
            self.finished = true;
        }
        if let Some(calls) = choice
            .pointer("/delta/tool_calls")
            .and_then(Value::as_array)
        {
            for call in calls {
                self.tool_delta(call);
            }
        }
        let delta = choice
            .pointer("/delta/content")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let out = delta.map(|d| {
            self.text.push_str(d);
            d.to_owned()
        });
        // One line can add at most two SSE lines' worth of bytes (text and tool arguments),
        // so checking once here bounds the total without checking inside every push.
        if self.kept() > self.cap {
            return Err(AiError::new(
                ErrorKind::Provider,
                "the streamed reply was too large",
            ));
        }
        Ok(out)
    }

    /// Appends one tool-call fragment to its slot, ignoring a hostile index.
    fn tool_delta(&mut self, call: &Value) {
        let index = call
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|i| usize::try_from(i).ok())
            .unwrap_or(0);
        if index >= MAX_TOOL_CALLS {
            return;
        }
        if self.tools.len() <= index {
            self.tools.resize(index + 1, (String::new(), String::new()));
        }
        if let Some(slot) = self.tools.get_mut(index) {
            if let Some(n) = call.pointer("/function/name").and_then(Value::as_str) {
                slot.0.push_str(n);
            }
            if let Some(a) = call.pointer("/function/arguments").and_then(Value::as_str) {
                slot.1.push_str(a);
            }
        }
    }

    /// The finished reply, once the stream has ended.
    pub fn into_completion(self) -> Completion {
        let tool_calls = self
            .tools
            .into_iter()
            .filter(|(name, _)| !name.is_empty())
            .map(|(n, a)| (n, if a.is_empty() { "{}".to_owned() } else { a }))
            .collect();
        Completion {
            text: self.text,
            tool_calls,
            usage: self.usage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text, tool calls, usage and the end markers all accumulate from whole lines.
    #[test]
    fn sse_accumulates_text_tools_and_usage() {
        let mut s = SseState::default();
        let lines = [
            ": keep-alive",
            "",
            "event: message",
            r#"data: {"choices":[{"delta":{"content":"Hel"}}]}"#,
            r#"data:{"choices":[{"delta":{"content":"lo"}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"f","arguments":"{\"a\""}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":1}"}}]},"finish_reason":"tool_calls"}]}"#,
            r#"data: {"choices":[],"usage":{"prompt_tokens":2,"completion_tokens":4}}"#,
            "data: [DONE]",
        ];
        let mut deltas = Vec::new();
        for l in lines {
            if let Some(d) = s.line(l).unwrap() {
                deltas.push(d);
            }
        }
        assert_eq!(deltas, ["Hel", "lo"]);
        assert!(s.done && s.finished);
        let c = s.into_completion();
        assert_eq!(c.text, "Hello");
        assert_eq!(c.tool_calls, [("f".into(), "{\"a\":1}".into())]);
        assert_eq!(c.usage.unwrap().input_units, Some(2));
    }

    /// Broken events, streamed errors and hostile indices are refused or ignored, not panics.
    #[test]
    fn sse_errors_and_garbage_are_errors_not_panics() {
        let mut s = SseState::default();
        assert!(s.line("data: {not json").is_err());
        assert!(
            s.line(r#"data: {"error":{"message":"overloaded"}}"#)
                .is_err()
        );
        // Hostile indices are ignored rather than allocated.
        s.line(r#"data: {"choices":[{"delta":{"tool_calls":[{"index":4000000000,"function":{"name":"x"}}]}}]}"#)
            .unwrap();
        assert!(s.into_completion().tool_calls.is_empty());
    }

    /// A stream past its total is refused, not grown; text and tool arguments share one cap.
    #[test]
    fn a_stream_past_its_total_is_refused_not_grown() {
        // The stream may never keep more than a whole reply could hold: the same cap as
        // http::MAX_BODY. Without it a hostile server streams deltas for as long as the
        // request timeout, and the reply grows without end.
        assert_eq!(SseState::default().cap, crate::http::MAX_BODY);
        let mut s = SseState::default().with_cap(10);
        s.line(r#"data: {"choices":[{"delta":{"content":"0123456789"}}]}"#)
            .unwrap();
        assert_eq!(
            s.line(r#"data: {"choices":[{"delta":{"content":"x"}}]}"#)
                .unwrap_err()
                .kind,
            ErrorKind::Provider
        );
        // Tool-call arguments count against the same total, not a separate one.
        let mut s = SseState::default().with_cap(10);
        s.line(r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"0123456789"}}]}}]}"#)
            .unwrap();
        assert!(
            s.line(r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"x"}}]}}]}"#)
                .is_err()
        );
    }
}
