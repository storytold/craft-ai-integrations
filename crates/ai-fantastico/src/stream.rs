//! Server-sent events from a chat-completions stream, put back together into the reply a
//! non-streamed request would have got.
//!
//! Every piece of message text goes to `on_text` as it arrives. Tool calls arrive in pieces keyed
//! by `index` and are assembled. An `error` event becomes the body, so the SDK classifies it as it
//! would a `200` carrying an error. `stop` is asked before every line: when it says yes, reading
//! ends at once with an error, which closes the connection and so ends the provider's work.

use std::io::{BufRead, Read};

use serde_json::{Map, Value, json};

/// A longer server-sent event line is refused: a chunk is a few hundred bytes.
pub(crate) const MAX_LINE_BYTES: usize = 1024 * 1024;
/// Most parallel tool calls kept in one reply; a higher index lands in the last slot.
pub(crate) const MAX_TOOL_CALLS: usize = 64;
/// Most bytes of text and tool calls kept from one stream: the cap a whole reply has
/// (`transport::MAX_BODY_BYTES`), so streaming is no way around it.
pub(crate) const MAX_ASSEMBLED_BYTES: usize = 16 * 1024 * 1024;

pub(crate) fn assemble(
    reader: impl BufRead,
    on_text: &mut dyn FnMut(&str),
    stop: &dyn Fn() -> bool,
) -> Result<Value, String> {
    assemble_capped(reader, on_text, stop, MAX_ASSEMBLED_BYTES)
}

fn assemble_capped(
    mut reader: impl BufRead,
    on_text: &mut dyn FnMut(&str),
    stop: &dyn Fn() -> bool,
    max_bytes: usize,
) -> Result<Value, String> {
    let mut a = Assembler {
        max: max_bytes,
        ..Assembler::default()
    };
    let mut line = String::new();
    let cap = u64::try_from(MAX_LINE_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    loop {
        if stop() {
            return Err("the job was stopped".into());
        }
        line.clear();
        let n = reader
            .by_ref()
            .take(cap)
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        if n == 0 {
            // The connection closed: a reply is whole only with its `[DONE]` (handled below) or
            // a finish reason. Anything else was cut short and is not passed off as an answer.
            if a.finish_reason.is_none() {
                return Err(
                    "the LLM provider ended the stream before the reply was complete".into(),
                );
            }
            break;
        }
        if line.len() > MAX_LINE_BYTES {
            return Err("the LLM provider sent an event too long to read".into());
        }
        let l = line.trim_end_matches(['\r', '\n']);
        let Some(data) = l.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        let Ok(chunk) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if chunk.get("error").is_some() {
            return Ok(chunk);
        }
        a.feed(&chunk, on_text);
        if a.over {
            return Err("the LLM provider sent a reply too large to keep".into());
        }
    }
    Ok(a.finish())
}

/// One tool call under construction.
#[derive(Default)]
struct ToolCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
struct Assembler {
    id: Option<Value>,
    model: Option<Value>,
    created: Option<Value>,
    role: Option<String>,
    content: String,
    has_content: bool,
    tool_calls: Vec<ToolCall>,
    finish_reason: Option<Value>,
    usage: Option<Value>,
    /// Bytes of text and tool-call pieces kept so far, and the most that may be kept.
    kept: usize,
    max: usize,
    /// A piece came that would pass `max`: it was dropped, and the reply is refused.
    over: bool,
}

impl Assembler {
    fn feed(&mut self, chunk: &Value, on_text: &mut dyn FnMut(&str)) {
        for (slot, key) in [
            (&mut self.id, "id"),
            (&mut self.model, "model"),
            (&mut self.created, "created"),
        ] {
            if slot.is_none()
                && let Some(v) = chunk.get(key).filter(|v| !v.is_null())
            {
                *slot = Some(v.clone());
            }
        }
        if let Some(u) = chunk.get("usage").filter(|u| u.is_object()) {
            self.usage = Some(u.clone());
        }
        let Some(choice) = chunk.pointer("/choices/0") else {
            return;
        };
        if let Some(f) = choice.get("finish_reason").filter(|f| !f.is_null()) {
            self.finish_reason = Some(f.clone());
        }
        let Some(delta) = choice.get("delta") else {
            return;
        };
        if let Some(r) = delta.get("role").and_then(Value::as_str) {
            self.role = Some(r.to_owned());
        }
        if let Some(t) = delta.get("content").and_then(Value::as_str) {
            self.has_content = true;
            if !t.is_empty() {
                if !self.room_for(t.len()) {
                    return;
                }
                self.content.push_str(t);
                on_text(t);
            }
        }
        for (n, call) in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let index = call
                .get("index")
                .and_then(Value::as_u64)
                .map_or(n, |i| usize::try_from(i).unwrap_or(n))
                .min(MAX_TOOL_CALLS);
            while self.tool_calls.len() <= index {
                self.tool_calls.push(ToolCall::default());
            }
            let Some(slot) = self.tool_calls.get_mut(index) else {
                continue;
            };
            if let Some(id) = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                slot.id = id.to_owned();
            }
            if let Some(name) = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                slot.name = name.to_owned();
            }
            if let Some(args) = call.pointer("/function/arguments").and_then(Value::as_str) {
                // As `room_for`, on the fields directly: `slot` borrows the tool calls.
                let kept = self.kept.saturating_add(args.len());
                if kept > self.max {
                    self.over = true;
                    return;
                }
                self.kept = kept;
                slot.arguments.push_str(args);
            }
        }
    }

    /// Whether `n` more bytes fit; counts them when they do. Checked before a piece is kept, so
    /// nothing past the cap is ever allocated.
    fn room_for(&mut self, n: usize) -> bool {
        let kept = self.kept.saturating_add(n);
        if kept > self.max {
            self.over = true;
            return false;
        }
        self.kept = kept;
        true
    }

    fn finish(self) -> Value {
        let mut message = Map::new();
        message.insert(
            "role".into(),
            json!(self.role.unwrap_or_else(|| "assistant".into())),
        );
        message.insert(
            "content".into(),
            if self.has_content || self.tool_calls.is_empty() {
                json!(self.content)
            } else {
                Value::Null
            },
        );
        if !self.tool_calls.is_empty() {
            let calls: Vec<Value> = self
                .tool_calls
                .into_iter()
                .enumerate()
                .map(|(i, c)| {
                    json!({
                        "id": if c.id.is_empty() { format!("call_{i}") } else { c.id },
                        "type": "function",
                        "function": {"name": c.name, "arguments": c.arguments}
                    })
                })
                .collect();
            message.insert("tool_calls".into(), Value::Array(calls));
        }
        let mut out = Map::new();
        out.insert("object".into(), json!("chat.completion"));
        for (key, v) in [
            ("id", self.id),
            ("model", self.model),
            ("created", self.created),
            ("usage", self.usage),
        ] {
            if let Some(v) = v {
                out.insert(key.into(), v);
            }
        }
        out.insert(
            "choices".into(),
            json!([{
                "index": 0,
                "message": message,
                "finish_reason": self.finish_reason.unwrap_or(Value::Null)
            }]),
        );
        Value::Object(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn stream(lines: &[&str]) -> (Value, Vec<String>) {
        let text = lines.join("\n") + "\n";
        let mut pieces = Vec::new();
        let v = assemble(text.as_bytes(), &mut |t| pieces.push(t.to_owned()), &|| {
            false
        })
        .unwrap();
        (v, pieces)
    }

    #[test]
    fn text_pieces_arrive_in_order_and_make_the_usual_reply() {
        let (v, pieces) = stream(&[
            ": keep-alive",
            r#"data: {"id":"gen-1","model":"m","created":7,"choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}"#,
            r#"data: {"id":"gen-1","choices":[{"index":0,"delta":{"content":"Hel"}}]}"#,
            "",
            r#"data: {"id":"gen-1","choices":[{"index":0,"delta":{"content":"lo."},"finish_reason":null}]}"#,
            r#"data: {"id":"gen-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"total_tokens":3,"cost":0.01}}"#,
            "data: [DONE]",
            r#"data: {"choices":[{"delta":{"content":"ignored after done"}}]}"#,
        ]);
        assert_eq!(pieces, ["Hel", "lo."]);
        assert_eq!(v["choices"][0]["message"]["content"], "Hello.");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
        assert_eq!(v["usage"]["cost"], 0.01);
        assert_eq!(
            (v["id"].as_str(), v["model"].as_str(), v["created"].as_u64()),
            (Some("gen-1"), Some("m"), Some(7))
        );
        assert!(v["choices"][0]["message"].get("tool_calls").is_none());
    }

    #[test]
    fn tool_calls_are_put_together_from_their_pieces() {
        let (v, pieces) = stream(&[
            r#"data: {"choices":[{"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"type_text","arguments":""}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"text\": \"Hi"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_b","function":{"name":"get_text","arguments":"{}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"}"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            "data: [DONE]",
        ]);
        assert!(pieces.is_empty());
        let m = &v["choices"][0]["message"];
        assert_eq!(m["content"], Value::Null);
        assert_eq!(m["tool_calls"][0]["id"], "call_a");
        assert_eq!(
            m["tool_calls"][0]["function"]["arguments"],
            r#"{"text": "Hi"}"#
        );
        assert_eq!(m["tool_calls"][1]["function"]["name"], "get_text");
    }

    #[test]
    fn an_error_event_becomes_the_body_and_junk_never_panics() {
        let (v, pieces) = stream(&[
            r#"data: {"choices":[{"delta":{"content":"par"}}]}"#,
            "data: not json at all",
            r#"data: {"error":{"message":"overloaded","code":503}}"#,
            r#"data: {"choices":[{"delta":{"content":"never"}}]}"#,
        ]);
        assert_eq!(pieces, ["par"]);
        assert_eq!(v["error"]["message"], "overloaded");
        let (v, _) = stream(&["data: [DONE]"]);
        assert_eq!(v["choices"][0]["message"]["content"], "");
        let (v, _) = stream(&[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":1e12,"function":{"name":"x"}}]}}]}"#,
            r#"data: {"choices":"nope"}"#,
            r#"data: {"choices":[{"delta":"nope"}]}"#,
            r#"data: 7"#,
            "data: [DONE]",
        ]);
        assert_eq!(
            v["choices"][0]["message"]["tool_calls"]
                .as_array()
                .map(Vec::len),
            Some(1)
        );
        let (v, _) = stream(&[
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":123456789,"function":{"name":"x"}}]}}]}"#,
            "data: [DONE]",
        ]);
        assert_eq!(
            v["choices"][0]["message"]["tool_calls"]
                .as_array()
                .map(Vec::len),
            Some(MAX_TOOL_CALLS + 1)
        );
        // A finished stream around the long line: only the line cap can refuse it.
        let long = format!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{}\"}}}}]}}\n{}\ndata: [DONE]\n",
            "x".repeat(MAX_LINE_BYTES),
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#
        );
        let e = assemble(long.as_bytes(), &mut |_| {}, &|| false).unwrap_err();
        assert!(e.contains("too long"), "{e}");
    }

    #[test]
    fn a_reply_too_large_to_keep_is_refused_whether_text_or_tool_arguments() {
        let text = "data: {\"choices\":[{\"delta\":{\"content\":\"12345\"}}]}
"
        .repeat(3)
            + "data: [DONE]\n";
        assert!(assemble_capped(text.as_bytes(), &mut |_| {}, &|| false, 15).is_ok());
        assert!(assemble_capped(text.as_bytes(), &mut |_| {}, &|| false, 14).is_err());
        let args = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"12345\"}}]}}]}
"
            .repeat(3);
        assert!(assemble_capped(args.as_bytes(), &mut |_| {}, &|| false, 14).is_err());
        assert!(MAX_ASSEMBLED_BYTES as u64 == crate::transport::MAX_BODY_BYTES);
    }

    /// A stream the connection cut short is an error, not a shorter answer; one that said how it
    /// finished is whole even without `[DONE]`.
    #[test]
    fn a_stream_cut_short_is_refused_and_a_finished_one_kept() {
        let piece = r#"data: {"choices":[{"delta":{"content":"Hel"}}]}"#;
        let cut = format!("{piece}\n");
        let e = assemble(cut.as_bytes(), &mut |_| {}, &|| false).unwrap_err();
        assert!(e.contains("before the reply was complete"), "{e}");
        assert!(assemble(&b""[..], &mut |_| {}, &|| false).is_err());
        let finished = format!(
            "{piece}\n{}\n",
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#
        );
        let v = assemble(finished.as_bytes(), &mut |_| {}, &|| false).unwrap();
        assert_eq!(v["choices"][0]["message"]["content"], "Hel");
    }

    #[test]
    fn a_piece_past_the_cap_is_never_kept_or_shown() {
        let mut shown = Vec::new();
        let big = r#"{"choices":[{"delta":{"content":"0123456789"}}]}"#;
        let mut a = Assembler {
            max: 4,
            ..Assembler::default()
        };
        a.feed(&serde_json::from_str(big).unwrap(), &mut |t| {
            shown.push(t.to_owned())
        });
        assert!(a.over && a.content.is_empty() && shown.is_empty());
        let args = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"0123456789"}}]}}]}"#;
        let mut a = Assembler {
            max: 4,
            ..Assembler::default()
        };
        a.feed(&serde_json::from_str(args).unwrap(), &mut |_| {});
        assert!(a.over);
        assert!(a.tool_calls.iter().all(|c| c.arguments.is_empty()));
    }

    #[test]
    fn a_stop_ends_reading_before_the_next_line() {
        let text = "data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\
                    data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n";
        let seen = Cell::new(0);
        let mut pieces = Vec::new();
        let r = assemble(text.as_bytes(), &mut |t| pieces.push(t.to_owned()), &|| {
            seen.set(seen.get() + 1);
            seen.get() > 1
        });
        assert!(r.is_err());
        assert_eq!(pieces, ["a"]);
    }
}
