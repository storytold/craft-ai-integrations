//! Same-origin Omni API calls. No cookies, redirects or key-bearing error text.
use craft_ai_core::{AiError, ErrorKind, SecretString};
use serde_json::Value;
use std::{io::Read, time::Duration};
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
    fn decode(&self, reply: Result<Response<Body>, ureq::Error>) -> Result<Value, AiError> {
        let mut response = reply
            .map_err(|_| AiError::new(ErrorKind::Network, "could not reach ArtCraft Omni API"))?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            let kind = match status {
                401 | 403 => ErrorKind::Auth,
                402 => ErrorKind::Quota,
                429 => ErrorKind::RateLimited,
                400 | 404 | 405 | 413 | 422 => ErrorKind::InvalidRequest,
                _ => ErrorKind::Provider,
            };
            return Err(AiError::new(
                kind,
                format!("ArtCraft Omni API answered {status}"),
            ));
        }
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| {
                AiError::new(
                    ErrorKind::Network,
                    "ArtCraft API response broke while reading",
                )
            })?;
        if bytes.len() > 1024 * 1024 {
            return Err(AiError::new(
                ErrorKind::Provider,
                "ArtCraft API response is too large",
            ));
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| AiError::new(ErrorKind::Provider, "ArtCraft API returned invalid JSON"))
    }
    /// Uploads a validated PNG as a private intermediate file, with no local filename disclosed.
    pub fn upload_png(&self, png: &[u8]) -> Result<String, AiError> {
        let boundary = format!("craft-{}", uuid::Uuid::new_v4());
        let mut data = Vec::new();
        for (name, value) in [
            ("uuid_idempotency_token", uuid::Uuid::new_v4().to_string()),
            ("maybe_visibility", "private".to_owned()),
            ("is_intermediate_system_file", "true".to_owned()),
        ] {
            data.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
        }
        data.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"craft-fill.png\"\r\nContent-Type: image/png\r\n\r\n").as_bytes());
        data.extend_from_slice(png);
        data.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let reply = self.decode(
            self.agent
                .post(format!("{}/v1/omni_api/upload/image", self.base))
                .header("Authorization", format!("Bearer {}", self.key.expose()))
                .content_type(format!("multipart/form-data; boundary={boundary}"))
                .send(&data),
        )?;
        let token = reply
            .get("media_file_token")
            .and_then(Value::as_str)
            .filter(|s| {
                s.starts_with("m_")
                    && s.len() <= 128
                    && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
            });
        if reply.get("success").and_then(Value::as_bool) != Some(true) || token.is_none() {
            return Err(AiError::new(
                ErrorKind::Provider,
                "ArtCraft did not return a valid uploaded image token",
            ));
        }
        token
            .map(str::to_owned)
            .ok_or_else(|| AiError::new(ErrorKind::Provider, "ArtCraft image upload failed"))
    }

    /// Bounded CDN image download, without auth, cookies or redirects. Loopback is test-only by base.
    pub fn image(&self, uri: &str) -> Result<craft_ai_core::Payload, AiError> {
        let bad = || {
            AiError::new(
                ErrorKind::Provider,
                "ArtCraft result image URL is not permitted",
            )
        };
        let url = url::Url::parse(uri).map_err(|_| bad())?;
        let local = craft_ai_core::endpoint::Endpoint::parse(&self.base)
            .is_ok_and(|e| e.is_loopback())
            && craft_ai_core::endpoint::Endpoint::parse(uri).is_ok_and(|e| e.is_loopback());
        if !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || !(url.scheme() == "https"
                && url.host_str() == Some("cdn-2.fakeyou.com")
                && url.port_or_known_default() == Some(443)
                || local && matches!(url.scheme(), "http" | "https"))
        {
            return Err(bad());
        }
        let mut response = self.agent.get(uri).call().map_err(|_| {
            AiError::new(
                ErrorKind::Network,
                "ArtCraft result image could not be downloaded",
            )
        })?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            // Busy or briefly down is worth another poll; anything else (gone, forbidden) is not.
            let (kind, message) = match status {
                429 => (
                    ErrorKind::RateLimited,
                    "ArtCraft result image download was rate limited",
                ),
                408 | 500..=599 => (
                    ErrorKind::Network,
                    "ArtCraft result image server is not answering right now",
                ),
                _ => (
                    ErrorKind::Provider,
                    "ArtCraft result image download was refused",
                ),
            };
            return Err(AiError::new(kind, message));
        }
        let limits = craft_ai_core::Limits::default();
        let limit = limits.max_inline_bytes;
        if response
            .headers()
            .get("content-length")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .is_some_and(|n| n > limit)
        {
            return Err(AiError::new(
                ErrorKind::Provider,
                "ArtCraft result image is too large",
            ));
        }
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| {
                AiError::new(
                    ErrorKind::Network,
                    "ArtCraft result image download ended early",
                )
            })?;
        let media = craft_ai_media::sniff::media_type(&bytes)
            .map(|m| m.0)
            .filter(|m| matches!(*m, "image/png" | "image/jpeg" | "image/webp"))
            .ok_or_else(|| {
                AiError::new(
                    ErrorKind::Provider,
                    "ArtCraft did not return a readable result image",
                )
            })?;
        let payload = craft_ai_core::Payload::inline(media, bytes)?;
        payload.check(&limits)?;
        Ok(payload)
    }

    pub fn get(&self, path: &str) -> Result<Value, AiError> {
        self.decode(
            self.agent
                .get(format!("{}{path}", self.base))
                .header("Authorization", format!("Bearer {}", self.key.expose()))
                .call(),
        )
    }
    pub fn post(&self, path: &str, body: &Value) -> Result<Value, AiError> {
        let bytes = serde_json::to_vec(body)
            .map_err(|_| AiError::new(ErrorKind::Internal, "could not encode ArtCraft request"))?;
        self.decode(
            self.agent
                .post(format!("{}{path}", self.base))
                .header("Authorization", format!("Bearer {}", self.key.expose()))
                .content_type("application/json")
                .send(&bytes),
        )
    }
}
