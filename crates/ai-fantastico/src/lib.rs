//! A FANTASTICO account as a provider for the Craft AI layer.
//!
//! [FANTASTICO](https://fantastico.cc) holds one prepaid credit with spending caps and fronts every
//! major LLM (Claude, GPT, Gemini, Llama…). Its account key cannot be sent to a model directly:
//! `fantastico-sdk` trades it for a short-lived, capped runtime key and sends the chat request
//! straight to the provider behind the account, at the address the account service gives with the
//! key, so the account service never sees a prompt. That
//! exchange (and its renewal, and `max_tokens` fitted to the cap) is why FANTASTICO needs this
//! adapter rather than a custom OpenAI-compatible connection or a declarative spec.
//!
//! | Capability | Notes |
//! |---|---|
//! | `text.complete` | streaming, tool calls, structured output, cost per request |
//!
//! An agent loop sends its whole conversation as [`TURN`] inputs (earlier replies and tool
//! results included) and reads the whole reply from the output labelled [`COMPLETION`]; see the
//! `chat` module. This is this adapter's own extension until the contract has turns.
//!
//! Not yet: images, speech, embeddings, image input to chat, cost estimates before a job. Each is
//! refused with a clear `Unsupported` error.
//!
//! # Using it
//!
//! In `craft-ai-integrations` the adapter is built in: `ProviderFactory::standard()` and
//! `::probing()` register it under [`PROVIDER_NAME`], so a connection saved with that provider name
//! and an account key (`ck-fant-…`) works in every app with no setup of its own. Registering it
//! again replaces the built-in one: an app does that to put its own name in the provider's usage
//! reports, and an app that takes this crate on its own does it to add the adapter at all:
//!
//! ```ignore
//! let mut factory = ProviderFactory::standard();
//! factory.register(craft_ai_fantastico::PROVIDER_NAME, craft_ai_fantastico::build_titled("WordCraft"));
//! ```
//!
//! Or build one directly:
//!
//! ```no_run
//! use craft_ai_core::{Provider, SecretString};
//! use craft_ai_fantastico::{FantasticoConfig, FantasticoProvider};
//!
//! let provider = FantasticoProvider::new(FantasticoConfig::new(SecretString::new("ck-fant-…")))?;
//! let models = provider.models()?;
//! # Ok::<(), craft_ai_core::AiError>(())
//! ```
//!
//! # Safety properties
//!
//! - The account key goes only to the account service; the runtime key only to the provider the
//!   SDK names. Redirects are never followed. Both keys stay out of `Debug` output and errors.
//! - Every call takes the Craft AI layer's TLS and proxy rules (`craft-ai-http`).
//! - Only listed request params are forwarded, so a param can never steer the URL, a key, the
//!   model or the account (see the `chat` module).
//! - Replies, stream lines, event backlogs and model lists are size-capped. No panics.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

mod chat;
mod errors;
mod jobs;
mod models;
mod stream;
mod transport;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use craft_ai_core::endpoint::Endpoint;
use craft_ai_core::payload::Limits;
use craft_ai_core::{
    AiError, Connection, ErrorKind, JobId, JobSnapshot, ModelId, ModelInfo, Provider, Request,
    SecretString,
};
use fantastico_sdk::{Fantastico, Options};

pub use crate::chat::{COMPLETION, TURN};
use crate::jobs::Registry;
use crate::transport::SdkTransport;
pub use crate::transport::{UreqWire, Wire, WireResponse};

/// The provider name a connection is saved under, and the adapter is registered under.
pub const PROVIDER_NAME: &str = "fantastico";

/// Settings for one FANTASTICO connection.
#[derive(Clone)]
pub struct FantasticoConfig {
    /// The account key (`ck-fant-…`), or a project key (`sk-fant-…`) on a trusted backend.
    pub account_key: SecretString,
    /// The account service, `https://fantastico.cc/api` when `None`.
    pub base_url: Option<String>,
    /// The application name in the provider's usage reports.
    pub title: String,
    /// The marketplace product, so the account service applies that product's terms.
    pub product: Option<String>,
    /// Use these models instead of asking the account service.
    pub models: Option<Vec<ModelInfo>>,
    /// The longest one request may take, retries included (the SDK's retry schedule).
    pub budget: Duration,
    /// The longest connecting may take; `None`: the HTTP stack's default.
    pub connect_timeout: Option<Duration>,
    /// The longest any single HTTP request may take; `None`: the SDK's own per-request times
    /// (30 s for the account service, 90 s for the model).
    pub request_timeout: Option<Duration>,
    /// Jobs that may run at once on this connection. Each running job holds its own runtime key,
    /// and each key reserves part of the account's credit, so this stays small.
    pub max_active_jobs: usize,
}

impl std::fmt::Debug for FantasticoConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FantasticoConfig")
            .field("base_url", &self.base_url)
            .field("title", &self.title)
            .field("product", &self.product)
            .field("max_active_jobs", &self.max_active_jobs)
            .finish_non_exhaustive()
    }
}

impl FantasticoConfig {
    pub fn new(account_key: SecretString) -> Self {
        Self {
            account_key,
            base_url: None,
            title: "Craft".to_owned(),
            product: None,
            models: None,
            budget: Duration::from_secs(300),
            connect_timeout: None,
            request_timeout: None,
            max_active_jobs: 2,
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn with_models(mut self, models: Vec<ModelInfo>) -> Self {
        self.models = Some(models);
        self
    }
}

/// SDK clients waiting for work. A client keeps its runtime key between jobs, so a key is minted
/// once per concurrent job slot rather than once per request.
struct Pool {
    key: SecretString,
    options: Options,
    wire: Arc<dyn Wire>,
    free: Mutex<Vec<Fantastico>>,
    keep: usize,
}

impl Pool {
    fn fresh(&self) -> Result<Fantastico, AiError> {
        Fantastico::with_transport(
            self.key.expose(),
            self.options.clone(),
            Box::new(SdkTransport::new(Arc::clone(&self.wire))),
        )
        .map_err(|e| errors::to_ai(&e, &self.key))
    }

    fn take(&self) -> Result<Fantastico, AiError> {
        let spare = self
            .free
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop();
        match spare {
            Some(c) => Ok(c),
            None => self.fresh(),
        }
    }

    fn give_back(&self, client: Fantastico) {
        let mut free = self.free.lock().unwrap_or_else(PoisonError::into_inner);
        if free.len() < self.keep {
            free.push(client);
        }
    }
}

/// The provider.
pub struct FantasticoProvider {
    pool: Arc<Pool>,
    models: Option<Vec<ModelInfo>>,
    limits: Limits,
    registry: Registry,
}

impl std::fmt::Debug for FantasticoProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FantasticoProvider")
    }
}

impl FantasticoProvider {
    /// Checks the key's shape and the base URL. Nothing is sent yet.
    pub fn new(config: FantasticoConfig) -> Result<Self, AiError> {
        let wire = UreqWire::with_timeouts(config.connect_timeout, config.request_timeout);
        Self::with_wire(config, Arc::new(wire))
    }

    /// As [`new`](Self::new), sending every request through `wire`: another HTTP stack, or a test
    /// double.
    pub fn with_wire(config: FantasticoConfig, wire: Arc<dyn Wire>) -> Result<Self, AiError> {
        let mut options = Options {
            title: config.title,
            product: config.product,
            budget: config.budget,
            ..Options::default()
        };
        if let Some(url) = config.base_url {
            let base = url.trim().trim_end_matches('/').to_owned();
            if base.contains(['?', '#']) {
                return Err(AiError::invalid(
                    "the FANTASTICO address must not have a query or fragment",
                ));
            }
            Endpoint::parse(&base)
                .map_err(|e| AiError::invalid(format!("FANTASTICO address: {e}")))?;
            options.base_url = base;
        }
        let keep = config.max_active_jobs.max(1);
        let pool = Pool {
            key: config.account_key,
            options,
            wire,
            free: Mutex::new(Vec::new()),
            keep,
        };
        // The SDK checks the key's shape without sending anything: refuse a wrong key now, and
        // keep the client for the first job.
        let first = pool.fresh()?;
        pool.give_back(first);
        Ok(Self {
            pool: Arc::new(pool),
            models: config.models,
            limits: Limits::default(),
            registry: Registry::new(keep),
        })
    }

    fn model_info(&self, id: &ModelId) -> Option<ModelInfo> {
        match &self.models {
            Some(list) => list.iter().find(|m| &m.id == id).cloned(),
            None => models::assumed(id),
        }
    }
}

impl Provider for FantasticoProvider {
    /// The account's chat models. Doubles as the connection test: the account service checks the
    /// key, and nothing is minted or spent.
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        if let Some(list) = &self.models {
            return Ok(list.clone());
        }
        let client = self.pool.take()?;
        let listed = client.models();
        self.pool.give_back(client);
        let entries = listed.map_err(|e| errors::to_ai(&e, &self.pool.key))?;
        Ok(models::from_list(&entries))
    }

    fn submit(&self, model: &ModelId, request: &Request) -> Result<JobId, AiError> {
        let info = self.model_info(model).ok_or_else(|| {
            AiError::unsupported(format!("model {model} is not offered on this connection"))
        })?;
        let spec = info.spec_for(&request.capability).ok_or_else(|| {
            AiError::unsupported(format!("model {model} cannot do {}", request.capability))
        })?;
        request.validate(spec, &self.limits)?;
        if request.capability.as_str() != "text.complete" {
            return Err(AiError::unsupported(format!(
                "this adapter does not serve {}",
                request.capability
            )));
        }
        let plan = chat::plan(model.as_str(), request)?;
        let pool = Arc::clone(&self.pool);
        self.registry.spawn(move |cell| {
            let mut client = pool.take()?;
            let reply = jobs::with_sink(cell, !plan.structured, || {
                client.chat_completion(&plan.body)
            });
            pool.give_back(client);
            let reply = reply.map_err(|e| errors::to_ai(&e, &pool.key))?;
            let completion = chat::parse_completion(&reply)?;
            for event in chat::events_for(&completion, plan.structured, plan.stream)? {
                cell.push(event);
            }
            if plan.conversation {
                cell.push(chat::completion_output(&reply)?);
            }
            Ok(completion.usage)
        })
    }

    fn poll(&self, job: &JobId) -> Result<JobSnapshot, AiError> {
        self.registry
            .get(job)
            .map(|cell| cell.snapshot())
            .ok_or_else(|| AiError::invalid(format!("unknown job {job}")))
    }

    fn cancel(&self, job: &JobId) -> Result<(), AiError> {
        self.registry
            .get(job)
            .map(|cell| cell.cancel())
            .ok_or_else(|| AiError::invalid(format!("unknown job {job}")))
    }
}

/// Builds the provider for a saved connection: the shape `ProviderFactory::register` takes.
///
/// The connection's `endpoint`, when set, replaces the account service's address (a staging
/// server, or a loopback test server). Models saved on the connection are the ones requests are
/// checked against, as for every other adapter.
pub fn build(
    connection: &Connection,
    key: Option<SecretString>,
) -> Result<Arc<dyn Provider>, AiError> {
    build_with(connection, key, None)
}

/// As [`build`], naming the app in the provider's usage reports: register
/// `build_titled("WordCraft")` instead of `build`.
pub fn build_titled(
    title: impl Into<String>,
) -> impl Fn(&Connection, Option<SecretString>) -> Result<Arc<dyn Provider>, AiError>
+ Send
+ Sync
+ 'static {
    let title = title.into();
    move |connection, key| build_with(connection, key, Some(title.clone()))
}

fn build_with(
    connection: &Connection,
    key: Option<SecretString>,
    title: Option<String>,
) -> Result<Arc<dyn Provider>, AiError> {
    let mut config = config_for(connection, key)?;
    if let Some(title) = title {
        config = config.with_title(title);
    }
    Ok(Arc::new(FantasticoProvider::new(config)?))
}

/// The settings [`build`] uses for a saved connection, for a caller that changes some (a time
/// budget, the app's name) before building the provider.
pub fn config_for(
    connection: &Connection,
    key: Option<SecretString>,
) -> Result<FantasticoConfig, AiError> {
    let key = key.filter(|k| !k.is_empty()).ok_or_else(|| {
        AiError::new(
            ErrorKind::Auth,
            format!(
                "connection \"{}\" needs a FANTASTICO account key (it starts with ck-fant-)",
                connection.label
            ),
        )
    })?;
    let mut config = FantasticoConfig::new(key);
    if let Some(endpoint) = &connection.endpoint {
        config = config.with_base_url(endpoint.clone());
    }
    if !connection.models.is_empty() {
        config = config.with_models(connection.models.clone());
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> SecretString {
        SecretString::new("ck-fant-needle")
    }

    #[test]
    fn the_key_shape_and_the_address_are_checked_before_anything_is_sent() {
        assert!(FantasticoProvider::new(FantasticoConfig::new(key())).is_ok());
        let wrong = FantasticoProvider::new(FantasticoConfig::new(SecretString::new("sk-other-x")));
        assert_eq!(wrong.unwrap_err().kind, ErrorKind::Auth);
        for (url, ok) in [
            ("https://fantastico.cc/api/", true),
            ("http://127.0.0.1:8080/api", true),
            ("http://fantastico.cc/api", false),
            ("https://fantastico.cc/api?x=1", false),
            ("https://u:p@fantastico.cc/api", false),
            ("", false),
        ] {
            let r = FantasticoProvider::new(FantasticoConfig::new(key()).with_base_url(url));
            assert_eq!(r.is_ok(), ok, "{url}");
        }
    }

    #[test]
    fn debug_never_shows_the_key() {
        let c = FantasticoConfig::new(key());
        assert!(!format!("{c:?}").contains("needle"));
        let p = FantasticoProvider::new(c).unwrap();
        assert!(!format!("{p:?}").contains("needle"));
    }

    fn connection(endpoint: Option<&str>) -> Connection {
        let mut v = serde_json::json!({
            "id": "fant", "label": "My FANTASTICO", "provider": PROVIDER_NAME,
            "locality": "remote", "auth": {"kind": "api_key", "secret": "fant-key"},
            "billing": "metered", "models": []
        });
        if let Some(e) = endpoint {
            v["endpoint"] = serde_json::json!(e);
        }
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn build_needs_a_key_and_honours_the_endpoint() {
        let c = connection(None);
        let e = build(&c, None).err().unwrap();
        assert_eq!(e.kind, ErrorKind::Auth);
        assert!(e.message.contains("My FANTASTICO"));
        assert!(build(&c, Some(SecretString::new(""))).is_err());
        assert!(build(&c, Some(key())).is_ok());
        assert!(build(&connection(Some("http://example.com")), Some(key())).is_err());
    }

    /// The factory's builder type, written out: `build` must keep fitting it.
    type BuildFn = dyn Fn(&Connection, Option<SecretString>) -> Result<Arc<dyn Provider>, AiError>
        + Send
        + Sync;

    #[test]
    fn build_fits_the_factory() {
        let f: Arc<BuildFn> = Arc::new(build);
        assert!(f(&connection(None), Some(key())).is_ok());
        let titled: Arc<BuildFn> = Arc::new(build_titled("WordCraft"));
        assert!(titled(&connection(None), Some(key())).is_ok());
    }
}
