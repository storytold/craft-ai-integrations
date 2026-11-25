//! Queue one graph on the local ComfyUI and wait for its history record.
use super::Sam3Session;
use craft_ai_core::{AiError, ErrorKind};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

impl Sam3Session {
    /// Best-effort removal of this request's own prompt after a cancellation: delete it if it is
    /// still queued and, only on a server confirmed to scope it, interrupt it if it is running.
    /// Nothing here names any other prompt or clears the queue, and a failure here is ignored so
    /// it cannot hide the cancellation.
    fn drop_prompt(&self, id: &str) {
        let _ = self.http.post("/queue", &json!({"delete": [id]}));
        if self.targeted_interrupt {
            let _ = self.http.post("/interrupt", &json!({"prompt_id": id}));
        }
    }

    /// Queues `graph` on the local ComfyUI and waits for its history record.
    /// Every sink is first routed to the scratch area with a fresh cache key (see
    /// `crate::scratch::route_sinks`), so a repeat request still returns an output and nothing
    /// is written to the user's `output/` folder.
    pub(super) fn queue(&self, mut graph: Value) -> Result<Value, AiError> {
        let started = self.canceller.mark();
        // Results go to ComfyUI's scratch area, and a fully cached sink can yield no history
        // outputs, so every sink gets a fresh cache key while source/model nodes stay cacheable.
        crate::scratch::route_sinks(&mut graph)?;
        let response = self.http.post("/prompt", &json!({"prompt":graph}))?;
        let id = response
            .get("prompt_id")
            .and_then(Value::as_str)
            .filter(|s| {
                !s.is_empty()
                    && s.len() <= 128
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            })
            .ok_or_else(|| {
                AiError::new(
                    ErrorKind::Provider,
                    "ComfyUI did not return a valid prompt id",
                )
            })?;
        let path = format!("/history/{id}");
        let deadline = Instant::now() + Duration::from_secs(240);
        loop {
            if self.canceller.fired_since(started) {
                self.drop_prompt(id);
                return Err(AiError::new(
                    ErrorKind::Cancelled,
                    "SAM 3 request cancelled",
                ));
            }
            let history = self.http.get_json(&path)?;
            if let Some(record) = history.get(id) {
                if record.pointer("/status/status_str").and_then(Value::as_str) == Some("error") {
                    return Err(AiError::new(
                        ErrorKind::Provider,
                        "SAM 3 workflow failed in ComfyUI",
                    ));
                }
                if record.pointer("/status/completed").and_then(Value::as_bool) == Some(true) {
                    return Ok(record.clone());
                }
            }
            if Instant::now() >= deadline {
                return Err(AiError::new(
                    ErrorKind::Network,
                    "SAM 3 timed out in ComfyUI",
                ));
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }
}
