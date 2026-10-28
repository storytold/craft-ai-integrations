//! Bounded, same-origin API calls. The key is never sent to result media URLs.
use craft_ai_core::{AiError, ErrorKind, SecretString};
use serde_json::Value;
use std::io::Read;
use std::time::Duration;
use ureq::{Body, http::Response};

pub(crate) struct Http {
    base: String,
    key: SecretString,
    agent: ureq::Agent,
}
impl Http {
    pub fn new(base: String, key: SecretString, connect: Duration, request: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .tls_config(craft_ai_http::tls_config())
            .proxy(craft_ai_http::proxy_for(&base))
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .timeout_connect(Some(connect))
            .timeout_global(Some(request))
            .build();
        Self {
            base,
            key,
            agent: ureq::Agent::new_with_config(config),
        }
    }
    fn response(&self, response: Result<Response<Body>, ureq::Error>) -> Result<Value, AiError> {
        let mut response =
            response.map_err(|_| AiError::new(ErrorKind::Network, "could not reach BFL"))?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok());
            return Err(status_error(status, retry_after));
        }
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| AiError::new(ErrorKind::Network, "BFL reply ended early"))?;
        if bytes.len() > 1024 * 1024 {
            return Err(AiError::new(ErrorKind::Provider, "BFL reply is too large"));
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| AiError::new(ErrorKind::Provider, "BFL returned invalid JSON"))
    }
    pub fn get(&self, path: &str) -> Result<Value, AiError> {
        self.response(
            self.agent
                .get(format!("{}{path}", self.base))
                .header("x-key", self.key.expose())
                .call(),
        )
    }
    /// Downloads a result picture. The key is not sent, redirects are not followed and the size
    /// is bounded by `limit`.
    pub fn download(&self, url: &str, limit: u64) -> Result<Vec<u8>, AiError> {
        let mut response =
            self.agent.get(url).call().map_err(|_| {
                AiError::new(ErrorKind::Network, "could not download the BFL result")
            })?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            return Err(AiError::new(
                ErrorKind::Provider,
                format!("the BFL result could not be downloaded ({status}); it may have expired"),
            ));
        }
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| AiError::new(ErrorKind::Network, "the BFL result download ended early"))?;
        if bytes.len() as u64 > limit {
            return Err(AiError::new(
                ErrorKind::Provider,
                "the BFL result is too large",
            ));
        }
        Ok(bytes)
    }
    pub fn post(&self, path: &str, value: &Value) -> Result<Value, AiError> {
        let bytes = serde_json::to_vec(value)
            .map_err(|_| AiError::new(ErrorKind::Internal, "could not encode request"))?;
        self.response(
            self.agent
                .post(format!("{}{path}", self.base))
                .header("x-key", self.key.expose())
                .content_type("application/json")
                .send(&bytes),
        )
    }
}

/// The error for a non-success answer. Only a rate limit carries the server's wait.
pub(crate) fn status_error(status: u16, retry_after: Option<u64>) -> AiError {
    let (kind, text) = match status {
        401 | 403 => (ErrorKind::Auth, "BFL refused the API key"),
        402 => (ErrorKind::Quota, "the BFL account is out of credits"),
        429 => (
            ErrorKind::RateLimited,
            "BFL is limiting requests; try again shortly",
        ),
        400 | 404 | 405 | 413 | 422 => (ErrorKind::InvalidRequest, "BFL refused the request"),
        _ => (ErrorKind::Provider, "BFL could not do it"),
    };
    let error = AiError::new(kind, format!("{text} ({status})"));
    match (status, retry_after) {
        (429, Some(secs)) => error.with_retry_after(secs),
        _ => error,
    }
}

pub(crate) fn encode_query(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
