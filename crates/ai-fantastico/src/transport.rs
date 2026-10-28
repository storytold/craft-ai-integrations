//! HTTP: the wire everything travels on, and the SDK transport on top of it that streams.
//!
//! `fantastico-sdk` sends every request through a [`Transport`] and reads replies whole. Ours
//! keeps that contract and adds streaming underneath: for a chat request with `"stream": true`
//! answered as server-sent events, it hands each piece of text to the job the calling thread is
//! working for (see [`crate::jobs::with_sink`]) and gives the SDK back the JSON a non-streamed
//! reply would carry. The SDK never knows.
//!
//! The [`Wire`] below the transport is replaceable, which is how the tests run without a network.
//! The real one, [`UreqWire`], takes the Craft AI layer's TLS and proxy rules from `craft-ai-http`
//! and never follows a redirect, so neither key can be forwarded to another host.

use std::io::{BufReader, Read};
use std::sync::Arc;
use std::time::Duration;

use fantastico_sdk::{HttpRequest, HttpResponse, Method, Transport};
use serde_json::Value;

use crate::jobs;
use crate::stream;

/// A whole reply larger than this is refused rather than read into memory (the SDK's own cap).
pub(crate) const MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;

/// A reply as it comes off the wire: the body is read by the caller, as fast as it arrives.
pub struct WireResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Box<dyn Read + Send>,
}

/// Sends one request and returns the reply unread.
///
/// Implement it for another HTTP stack or a test double. An HTTP error status is a reply, not an
/// `Err`; `Err` is for DNS, TLS, connection and timeout failures, as text.
pub trait Wire: Send + Sync {
    fn send(&self, request: &HttpRequest) -> Result<WireResponse, String>;
}

/// The real wire: blocking `ureq` with the platform's certificate check and no redirects.
pub struct UreqWire {
    agent: ureq::Agent,
    /// The longest any one request may take, over the time the SDK gives it.
    request_cap: Option<Duration>,
}

impl Default for UreqWire {
    fn default() -> Self {
        Self::with_timeouts(None, None)
    }
}

impl UreqWire {
    /// `connect` bounds connecting; `request` caps each request, whose own time the SDK sets (the
    /// shorter of the two wins). `None` leaves the SDK's times as they are.
    pub fn with_timeouts(connect: Option<Duration>, request: Option<Duration>) -> Self {
        let config = ureq::Agent::config_builder()
            .tls_config(craft_ai_http::tls_config())
            .proxy(craft_ai_http::hosted_proxy())
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .timeout_connect(connect)
            .build();
        Self {
            agent: config.into(),
            request_cap: request,
        }
    }
}

/// A request's time: its own, or the cap when that is shorter.
pub(crate) fn capped(own: Duration, cap: Option<Duration>) -> Duration {
    cap.map_or(own, |c| own.min(c))
}

impl std::fmt::Debug for UreqWire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UreqWire")
    }
}

fn with_headers<B>(
    mut builder: ureq::RequestBuilder<B>,
    request: &HttpRequest,
    cap: Option<Duration>,
) -> ureq::RequestBuilder<B> {
    for (k, v) in &request.headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    builder
        .config()
        .timeout_global(Some(capped(request.timeout, cap)))
        .build()
}

impl Wire for UreqWire {
    fn send(&self, request: &HttpRequest) -> Result<WireResponse, String> {
        let url = request.url.as_str();
        let result = match request.method {
            Method::Get => with_headers(self.agent.get(url), request, self.request_cap).call(),
            Method::Delete => {
                with_headers(self.agent.delete(url), request, self.request_cap).call()
            }
            Method::Post => with_headers(self.agent.post(url), request, self.request_cap)
                .send(request.body.as_deref().unwrap_or("")),
        };
        let response = result.map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|v| (k.as_str().to_owned(), v.to_owned()))
            })
            .collect();
        // The stream itself is capped per line by the reader; the whole body by the caller.
        let body = response.into_body().into_reader();
        Ok(WireResponse {
            status,
            headers,
            body: Box::new(body),
        })
    }
}

/// The SDK's transport: whole replies as it expects, streaming underneath when asked.
pub(crate) struct SdkTransport {
    wire: Arc<dyn Wire>,
}

impl SdkTransport {
    pub fn new(wire: Arc<dyn Wire>) -> Self {
        Self { wire }
    }
}

/// Whether `request` asks for a streamed reply.
pub(crate) fn wants_stream(request: &HttpRequest) -> bool {
    request.method == Method::Post
        && request
            .body
            .as_deref()
            .and_then(|b| serde_json::from_str::<Value>(b).ok())
            .is_some_and(|v| v.get("stream").and_then(Value::as_bool) == Some(true))
}

fn is_event_stream(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("content-type")
            && v.to_ascii_lowercase().contains("text/event-stream")
    })
}

impl Transport for SdkTransport {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
        let WireResponse {
            status,
            headers,
            body,
        } = self.wire.send(request)?;
        // Streamed only when asked and answered as a stream: an error status or a plain JSON
        // reply is read whole, as the SDK expects.
        if !(wants_stream(request) && (200..300).contains(&status) && is_event_stream(&headers)) {
            let mut text = String::new();
            let read = body
                .take(MAX_BODY_BYTES.saturating_add(1))
                .read_to_string(&mut text)
                .map_err(|e| e.to_string())?;
            if u64::try_from(read).unwrap_or(u64::MAX) > MAX_BODY_BYTES {
                return Err("the reply is too large to read".into());
            }
            return Ok(HttpResponse {
                status,
                headers,
                body: text,
            });
        }
        let assembled = stream::assemble(BufReader::new(body), &mut jobs::emit_text, &|| {
            jobs::sink_stopped()
        })?;
        Ok(HttpResponse {
            status,
            headers,
            body: assembled.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn req(method: Method, body: Option<&str>) -> HttpRequest {
        HttpRequest {
            method,
            url: "https://example.org".into(),
            headers: vec![],
            body: body.map(str::to_owned),
            timeout: Duration::from_secs(1),
        }
    }

    #[test]
    fn only_a_streamed_post_streams() {
        assert!(wants_stream(&req(
            Method::Post,
            Some(r#"{"stream": true, "messages": []}"#)
        )));
        assert!(!wants_stream(&req(
            Method::Post,
            Some(r#"{"stream": false}"#)
        )));
        assert!(!wants_stream(&req(Method::Post, Some("{not json"))));
        assert!(!wants_stream(&req(Method::Get, None)));
    }

    #[test]
    fn a_request_takes_the_shorter_of_its_own_time_and_the_cap() {
        let s = Duration::from_secs;
        assert_eq!(capped(s(90), Some(s(20))), s(20));
        assert_eq!(capped(s(30), Some(s(600))), s(30));
        assert_eq!(capped(s(90), None), s(90));
        // Built either way without a request being sent.
        let _ = UreqWire::with_timeouts(Some(s(5)), Some(s(20)));
        let _ = UreqWire::default();
    }

    #[test]
    fn the_content_type_decides_whether_a_reply_is_a_stream() {
        let h = |v: &str| vec![("Content-Type".to_owned(), v.to_owned())];
        assert!(is_event_stream(&h("text/event-stream; charset=utf-8")));
        assert!(!is_event_stream(&h("application/json")));
        assert!(!is_event_stream(&[]));
    }

    struct Canned(u16, &'static str, &'static str);

    impl Wire for Canned {
        fn send(&self, _: &HttpRequest) -> Result<WireResponse, String> {
            Ok(WireResponse {
                status: self.0,
                headers: vec![("content-type".into(), self.1.into())],
                body: Box::new(self.2.as_bytes()),
            })
        }
    }

    #[test]
    fn an_error_status_is_read_whole_even_when_a_stream_was_asked_for() {
        let t = SdkTransport::new(Arc::new(Canned(
            429,
            "text/event-stream",
            r#"{"error":"slow"}"#,
        )));
        let r = t
            .send(&req(Method::Post, Some(r#"{"stream":true}"#)))
            .unwrap();
        assert_eq!((r.status, r.body.as_str()), (429, r#"{"error":"slow"}"#));
    }

    #[test]
    fn a_stream_comes_back_as_one_reply() {
        let t = SdkTransport::new(Arc::new(Canned(
            200,
            "text/event-stream",
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n",
        )));
        let r = t
            .send(&req(Method::Post, Some(r#"{"stream":true}"#)))
            .unwrap();
        let v: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["choices"][0]["message"]["content"], "hi");
    }
}
