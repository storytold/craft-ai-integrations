//! The FANTASTICO account service, and the runtime keys it mints.
//!
//! The account key never reaches a model. [`Account`] trades it for a short-lived, capped runtime
//! key (`POST /edge/key`, or `/runtime/key` for a project key), keeps that key until a minute
//! before it expires, and sends chat requests with it straight to the provider at the address the
//! account service gave with the key, so the account service never sees a prompt. Around each chat
//! request it:
//!
//! - merges the fields the account service asked for with the key under the request's own;
//! - fills in `max_tokens` from the key's spending cap when the request names none, because the
//!   provider checks the worst-case cost against the cap before generating anything;
//! - retries the same request after `429` and `503`, honouring `Retry-After`, within the budget;
//! - mints a new key and retries **once** after `401` and `402`: the key may have been revoked, or
//!   its cap reached before it expired.
//!
//! The account routes (`/config`, `/balance`, `/models`) take the account key and mint nothing.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use craft_ai_core::SecretString;
use serde_json::{Map, Value, json};

use crate::errors::Error;
use crate::provider_error::{ProviderError, empty_completion};
use crate::time::{now_ms, parse_iso_ms};
use crate::transport::{HttpRequest, HttpResponse, Method, Transport};

/// The root of the FANTASTICO API.
pub(crate) const DEFAULT_BASE_URL: &str = "https://fantastico.cc/api";
/// A key that expires within this margin is replaced rather than reused.
const EXPIRY_SKEW_MS: i64 = 60_000;
/// How long the model price list is kept.
const PRICES_TTL: Duration = Duration::from_secs(5 * 60);
pub(crate) const CLOUD_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const PROVIDER_TIMEOUT: Duration = Duration::from_secs(90);
/// The address shown with the app's name in the provider's usage reports (`HTTP-Referer`).
const REFERER: &str = "https://fantastico.cc";
/// Retries after the first attempt of a rate-limited or overloaded request.
const RETRIES: u32 = 2;
/// The first wait when the provider doesn't say how long; it doubles on each retry.
const BASE_DELAY_MS: u64 = 500;
/// The longest single wait.
const MAX_DELAY_MS: u64 = 10_000;
/// The share of a key's cap one request may plan to spend.
const HEADROOM: f64 = 0.8;
/// Above this many affordable tokens the cap is roomy enough to leave `max_tokens` unset.
const COMFORTABLE: f64 = 8192.0;
/// The smallest `max_tokens` worth sending.
const FLOOR: u64 = 64;

/// How an [`Account`] behaves.
#[derive(Clone, Debug)]
pub(crate) struct Options {
    /// The root of the FANTASTICO API, without a trailing `/`.
    pub base_url: String,
    /// The marketplace product, sent with every key request so the account service applies that
    /// product's terms.
    pub product: Option<String>,
    /// The app's name in the provider's usage reports (`X-Title`).
    pub title: String,
    /// The longest one chat request may spend on the provider, retries and waits included.
    /// Minting a key, when one is needed, is not counted.
    pub budget: Duration,
}

/// Which API surface a credential talks to: the key's prefix decides it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Surface {
    /// `/runtime`, reached with a project key (`sk-fant-…`) on a trusted backend.
    Runtime,
    /// `/edge`, reached with an account key (`ck-fant-…`).
    Edge,
}

impl Surface {
    fn of(key: &str) -> Result<Self, Error> {
        if key.starts_with("sk-fant-") {
            Ok(Self::Runtime)
        } else if key.starts_with("ck-fant-") {
            Ok(Self::Edge)
        } else {
            Err(Error::new(
                "This is not a FANTASTICO key: an account key starts with \"ck-fant-\", a project key with \"sk-fant-\"",
                "UNKNOWN_CREDENTIAL",
                400,
            ))
        }
    }

    fn path(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Edge => "edge",
        }
    }
}

/// An account's credit.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Balance {
    /// What can still be spent, in USD. Part of the account's balance may already be reserved by
    /// a live key, so this is the available amount, not the balance.
    pub remaining_usd: f64,
    /// Unspent reservations across all open keys.
    pub reserved_unspent_usd: Option<f64>,
    /// What is left including unspent reservations, as the account service works it out: the
    /// amount to show a person. Absent on older servers.
    pub total_remaining_usd: Option<f64>,
    /// A top-up warning on the total, when the server gives one.
    pub low_total_balance: Option<bool>,
    /// Whether the credit is below the account service's top-up threshold.
    pub low_balance: bool,
    /// The account's state, such as `active` or `suspended`, when reported.
    pub status: Option<String>,
}

/// The account behind an account key.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct AccountInfo {
    /// The account's identifier. Not a secret: it can be shown and recorded in place of the key.
    pub account_id: String,
    /// `active` or `suspended`.
    pub status: String,
    /// The model the account service suggests for this account, if any.
    pub model: Option<String>,
    /// The account's credit, or `None` when it could not be read.
    pub balance: Option<Balance>,
}

/// The runtime key in hand, with where to spend it and on which terms: both come from the same
/// mint, and a replacement key brings its own.
struct RuntimeKey {
    secret: SecretString,
    /// The OpenAI-compatible address the key is accepted at, without a trailing `/`.
    base_url: String,
    /// The model for requests that name none, as the account service decided.
    default_model: Option<String>,
    /// Fields merged into every chat body under the ones the request writes.
    chat_defaults: Map<String, Value>,
    expires_at_ms: i64,
    limit_usd: Option<f64>,
}

/// How one attempt at the provider ended.
enum Failure {
    /// The provider refused: the caller decides whether a new key may help.
    Provider(ProviderError),
    /// Anything else, already in the form the caller sees.
    Final(Error),
}

/// One credential's client: the account key, the runtime key minted for it, and the price list.
/// Cheap to keep: it holds the runtime key between calls.
pub(crate) struct Account {
    key: SecretString,
    surface: Surface,
    opts: Options,
    transport: Transport,
    sleep: Box<dyn Fn(Duration) + Send + Sync>,
    runtime: Option<RuntimeKey>,
    /// The activity session the account service groups this client's spending under; sent back
    /// with every later mint so a replacement key keeps the conversation's terms.
    activity_session_id: Option<String>,
    prices: Option<(Instant, HashMap<String, f64>)>,
    rng: u64,
}

fn str_field(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// A number sent as a JSON number or a decimal string; never NaN or infinite.
fn finite(v: Option<&Value>) -> Option<f64> {
    let n = match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }?;
    n.is_finite().then_some(n)
}

fn balance_of(v: &Value) -> Option<Balance> {
    Some(Balance {
        remaining_usd: finite(v.get("remainingUsd"))?,
        reserved_unspent_usd: finite(v.get("reservedUnspentUsd")),
        total_remaining_usd: finite(v.get("totalRemainingUsd")),
        low_total_balance: v.get("lowTotalBalance").and_then(Value::as_bool),
        low_balance: v
            .get("lowBalance")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        status: str_field(v, "status"),
    })
}

/// The `max_tokens` to send, or `None` to leave it unset: without a cap or a price there is
/// nothing to compute, and a guess would be worse than silence.
fn affordable(limit_usd: Option<f64>, price_per_output_token: Option<f64>) -> Option<u64> {
    let limit = limit_usd.filter(|l| l.is_finite() && *l > 0.0)?;
    let price = price_per_output_token.filter(|p| p.is_finite() && *p > 0.0)?;
    let n = ((limit * HEADROOM) / price).floor();
    if !n.is_finite() || n >= COMFORTABLE {
        return None;
    }
    // `n` is finite and in [0, 8192): the conversion is exact.
    Some((n.max(0.0) as u64).max(FLOOR))
}

/// The wait before retry number `attempt` (counting from 1). The provider's `Retry-After` wins;
/// otherwise the wait doubles. Both are capped, then jittered over the upper half of the window
/// (50% to 100%, `random` in `[0, 1)`), so clients refused together don't retry together, and the
/// wait never drops below half of what the provider asked for.
fn delay_for(err: &ProviderError, attempt: u32, random: f64) -> Duration {
    let wanted = err
        .retry_after_ms
        .unwrap_or_else(|| BASE_DELAY_MS.saturating_mul(1u64 << attempt.saturating_sub(1).min(20)));
    let capped = wanted.min(MAX_DELAY_MS);
    let factor = 0.5 + random.clamp(0.0, 1.0) * 0.5;
    Duration::from_millis((capped as f64 * factor).round() as u64)
}

impl Account {
    /// A client for `key`. Nothing is sent.
    ///
    /// # Errors
    ///
    /// `UNKNOWN_CREDENTIAL` when the key is neither an account key nor a project key.
    pub fn new(key: SecretString, opts: Options, transport: Transport) -> Result<Self, Error> {
        let key = SecretString::new(key.expose().trim());
        let surface = Surface::of(key.expose())?;
        let seed = u64::try_from(now_ms()).unwrap_or(1) ^ 0x9E37_79B9_7F4A_7C15;
        Ok(Self {
            key,
            surface,
            opts: Options {
                base_url: opts.base_url.trim_end_matches('/').to_owned(),
                ..opts
            },
            transport,
            sleep: Box::new(std::thread::sleep),
            runtime: None,
            activity_session_id: None,
            prices: None,
            rng: seed | 1,
        })
    }

    /// Replaces how the client waits between retries.
    #[cfg(test)]
    pub fn with_sleep(mut self, sleep: impl Fn(Duration) + Send + Sync + 'static) -> Self {
        self.sleep = Box::new(sleep);
        self
    }

    // ── Account service ──────────────────────────────────────────────────────────────────

    /// A call to the account service with the account key. A refusal carries the service's
    /// `message` and `type`, or `fallback` as the code when the body has none.
    fn api(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        fallback: &str,
    ) -> Result<Value, Error> {
        let mut headers = vec![(
            "Authorization".to_owned(),
            format!("Bearer {}", self.key.expose()),
        )];
        if body.is_some() {
            headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
        }
        let req = HttpRequest {
            method,
            url: format!("{}/{}{path}", self.opts.base_url, self.surface.path()),
            headers,
            body: body.map(|b| b.to_string()),
            timeout: CLOUD_TIMEOUT,
        };
        let res = self
            .transport
            .send(&req)
            .map_err(|e| Error::new(e, "NETWORK_ERROR", 503))?;
        let json = res.json();
        if !res.ok() {
            let b = json.unwrap_or(Value::Null);
            let message =
                str_field(&b, "message").unwrap_or_else(|| format!("{fallback} ({})", res.status));
            let code = str_field(&b, "type").unwrap_or_else(|| fallback.to_owned());
            return Err(Error::new(message, &code, res.status));
        }
        json.ok_or_else(|| {
            Error::new(
                "the account service sent an unreadable reply",
                "INVALID_RESPONSE",
                502,
            )
        })
    }

    fn edge_only(&self, what: &str) -> Result<(), Error> {
        if self.surface == Surface::Edge {
            return Ok(());
        }
        Err(Error::new(
            format!("{what} needs an account key (ck-fant-…), not a project key"),
            "SURFACE_UNSUPPORTED",
            400,
        ))
    }

    /// The account behind the key. It checks the key without minting anything.
    ///
    /// # Errors
    ///
    /// `SURFACE_UNSUPPORTED` with a project key (nothing is sent), the account service's code when
    /// it refuses the key (such as `INVALID_CONTO_KEY`), or `NETWORK_ERROR`.
    pub fn info(&self) -> Result<AccountInfo, Error> {
        self.edge_only("Reading the account")?;
        let v = self.api(Method::Get, "/config", None, "CONFIG_FAILED")?;
        let account_id = str_field(&v, "contoId").ok_or_else(|| {
            Error::new(
                "the account configuration is incomplete",
                "INVALID_RESPONSE",
                502,
            )
        })?;
        Ok(AccountInfo {
            account_id,
            status: str_field(&v, "status").unwrap_or_else(|| "active".to_owned()),
            model: str_field(&v, "model"),
            balance: v.get("balance").and_then(balance_of),
        })
    }

    /// The account's credit.
    ///
    /// # Errors
    ///
    /// As [`Self::info`].
    pub fn balance(&self) -> Result<Balance, Error> {
        self.edge_only("Reading the account's credit")?;
        let v = self.api(Method::Get, "/balance", None, "BALANCE_FAILED")?;
        balance_of(&v).ok_or_else(|| {
            Error::new(
                "the account's credit is unreadable",
                "INVALID_RESPONSE",
                502,
            )
        })
    }

    /// The models this credential may use, with their prices, as the account service lists them.
    pub fn models(&self) -> Result<Vec<Value>, Error> {
        let v = self.api(Method::Get, "/models", None, "MODELS_FAILED")?;
        Ok(v.get("models")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    // ── Runtime key ──────────────────────────────────────────────────────────────────────

    fn mint_if_needed(&mut self) -> Result<(), Error> {
        if self
            .runtime
            .as_ref()
            .is_some_and(|k| k.expires_at_ms.saturating_sub(now_ms()) > EXPIRY_SKEW_MS)
        {
            return Ok(());
        }
        self.runtime = None;
        // The product and the session go with every mint, not only the first: a key can be
        // replaced mid-conversation, and a replacement minted without them would change the terms
        // of a conversation already under way.
        let mut body = Map::new();
        if let Some(p) = &self.opts.product {
            body.insert("product".into(), json!(p));
        }
        if let Some(s) = &self.activity_session_id {
            body.insert("activitySessionId".into(), json!(s));
        }
        let data = self.api(
            Method::Post,
            "/key",
            Some(Value::Object(body)),
            "MINT_FAILED",
        )?;
        let incomplete = || {
            Error::new(
                "the account service sent an incomplete key",
                "MINT_FAILED",
                502,
            )
        };
        let secret = str_field(&data, "runtimeKey").ok_or_else(incomplete)?;
        let expires_at_ms = str_field(&data, "expiresAt")
            .and_then(|at| parse_iso_ms(&at))
            .ok_or_else(incomplete)?;
        // No fallback address: a key spent somewhere the account service didn't name would be a
        // guess, and there is nothing to guess from.
        let base_url = str_field(&data, "baseUrl")
            .map(|u| u.trim_end_matches('/').to_owned())
            .filter(|u| !u.is_empty())
            .ok_or_else(|| {
                Error::new(
                    "the account service didn't say where to use the key (no baseUrl)",
                    "PROVIDER_ACCESS_MISSING",
                    502,
                )
            })?;
        if let Some(s) = str_field(&data, "activitySessionId") {
            self.activity_session_id = Some(s);
        }
        self.runtime = Some(RuntimeKey {
            secret: SecretString::new(secret),
            base_url,
            default_model: str_field(&data, "defaultModel"),
            chat_defaults: data
                .get("chatDefaults")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            expires_at_ms,
            limit_usd: finite(data.get("limit")).filter(|l| *l >= 0.0),
        });
        Ok(())
    }

    /// The USD price of one output token of `model`, from the price list (kept five minutes).
    fn output_price(&mut self, model: &str) -> Option<f64> {
        if self
            .prices
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() > PRICES_TTL)
        {
            let models = self.models().ok()?;
            let by_id = models
                .iter()
                .filter_map(|m| {
                    Some((
                        str_field(m, "id")?,
                        finite(m.pointer("/pricing/completion")).filter(|p| *p > 0.0)?,
                    ))
                })
                .collect();
            self.prices = Some((Instant::now(), by_id));
        }
        self.prices
            .as_ref()
            .and_then(|(_, m)| m.get(model).copied())
    }

    /// A number in `[0, 1)` for retry jitter (xorshift64*: enough for jitter, nothing more).
    fn random(&mut self) -> f64 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    // ── Inference ────────────────────────────────────────────────────────────────────────

    /// Sends an OpenAI-style chat-completions body to the provider with the runtime key, and
    /// returns the provider's reply (see the module docs for what is done around it).
    ///
    /// # Errors
    ///
    /// - `INVALID_REQUEST` when `request` is not a JSON object; `MODEL_REQUIRED` when neither the
    ///   request nor the account service names a model.
    /// - The account service's refusal to mint a key, such as `ALLOWANCE_EXHAUSTED`, or
    ///   `MINT_FAILED` and `PROVIDER_ACCESS_MISSING` for an incomplete key.
    /// - `PROVIDER_<TYPE>` when the provider classified its refusal, `LLM_PROVIDER_ERROR` when a
    ///   new key didn't help either, `INTERNAL_ERROR` for an unclassified failure.
    /// - `EMPTY_COMPLETION` when the model used its whole output budget and returned nothing.
    /// - `TIMEOUT` when the budget ran out before a request could be sent.
    /// - `NETWORK_ERROR` or `INVALID_RESPONSE` for transport and protocol failures.
    pub fn chat_completion(&mut self, request: &Value) -> Result<Value, Error> {
        let Some(obj) = request.as_object() else {
            return Err(Error::new(
                "the request must be a JSON object",
                "INVALID_REQUEST",
                400,
            ));
        };
        let mut body = obj.clone();
        let explicit_max = body.get("max_tokens").is_some_and(|v| !v.is_null());
        let deadline = Instant::now() + self.opts.budget;
        // A refused mint is the account service's verdict: pass it on.
        self.mint_if_needed()?;
        // The model is chosen once, before the first attempt: a replacement key carries on the
        // same conversation, with the same model.
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty())
            .map(str::to_owned)
            .or_else(|| self.runtime.as_ref().and_then(|k| k.default_model.clone()))
            .ok_or_else(|| {
                Error::new(
                    "no model: the request names none, and neither does the account",
                    "MODEL_REQUIRED",
                    400,
                )
            })?;
        body.insert("model".into(), json!(model));
        let first = match self.attempt(&body, &model, explicit_max, deadline) {
            Ok(v) => return Ok(v),
            Err(Failure::Final(e)) => return Err(e),
            Err(Failure::Provider(p)) => p,
        };
        // A new key doesn't fix a moderation block or a rate limit that outlasted the retries:
        // don't spend one.
        if !first.is_remintable() {
            return Err(first.into_error());
        }
        self.runtime = None;
        self.mint_if_needed()?;
        match self.attempt(&body, &model, explicit_max, deadline) {
            Ok(v) => Ok(v),
            Err(Failure::Final(e)) => Err(e),
            // The second attempt can fail for another reason: if the provider named it, say so.
            Err(Failure::Provider(second)) => match second.named() {
                Some(e) if !second.is_remintable() => Err(e),
                _ => Err(Error::new(
                    "the model's provider kept refusing the request",
                    "LLM_PROVIDER_ERROR",
                    502,
                )),
            },
        }
    }

    /// One attempt with the key in hand, retrying transient failures around the single request.
    /// `max_tokens`, the address and the default fields are taken here, after minting, because a
    /// new key brings its own cap and its own terms.
    fn attempt(
        &mut self,
        request: &Map<String, Value>,
        model: &str,
        explicit_max: bool,
        deadline: Instant,
    ) -> Result<Value, Failure> {
        let Some((secret, base_url, mut body, limit)) = self.runtime.as_ref().map(|k| {
            (
                k.secret.clone(),
                k.base_url.clone(),
                k.chat_defaults.clone(),
                k.limit_usd,
            )
        }) else {
            return Err(Failure::Final(Error::new(
                "no runtime key in hand",
                "MINT_FAILED",
                402,
            )));
        };
        // The account service's fields first, then the request's: what the caller writes wins.
        body.extend(request.iter().map(|(k, v)| (k.clone(), v.clone())));
        if !explicit_max {
            let price = limit.and_then(|_| self.output_price(model));
            if let Some(n) = affordable(limit, price) {
                body.insert("max_tokens".into(), json!(n));
            }
        }
        let max_tokens = body.get("max_tokens").and_then(Value::as_u64);
        let mut request = HttpRequest {
            method: Method::Post,
            url: format!("{base_url}/chat/completions"),
            headers: vec![
                (
                    "Authorization".to_owned(),
                    format!("Bearer {}", secret.expose()),
                ),
                ("Content-Type".to_owned(), "application/json".to_owned()),
                ("HTTP-Referer".to_owned(), REFERER.to_owned()),
                ("X-Title".to_owned(), self.opts.title.clone()),
            ],
            body: Some(Value::Object(body).to_string()),
            timeout: PROVIDER_TIMEOUT,
        };
        let mut retries = 0;
        loop {
            // The budget bounds the requests too, not only the waits between them.
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Failure::Final(Error::new(
                    "the request ran out of time before reaching the model's provider",
                    "TIMEOUT",
                    504,
                )));
            }
            request.timeout = left.min(PROVIDER_TIMEOUT);
            let outcome = match self.transport.send(&request) {
                Ok(res) => classify(&res, model, max_tokens),
                Err(e) => Err(Failure::Final(Error::new(
                    format!("the model's provider cannot be reached: {e}"),
                    "NETWORK_ERROR",
                    503,
                ))),
            };
            let err = match outcome {
                Err(Failure::Provider(p)) if p.is_transient() && retries < RETRIES => p,
                other => return other,
            };
            let random = self.random();
            let delay = delay_for(&err, retries + 1, random);
            // Never sleep past the budget: reporting the rate limit is more useful than a timeout.
            if Instant::now() + delay > deadline {
                return Err(Failure::Provider(err));
            }
            (self.sleep)(delay);
            retries += 1;
        }
    }
}

/// The provider's reply: a structured failure, or the completion.
fn classify(res: &HttpResponse, model: &str, max_tokens: Option<u64>) -> Result<Value, Failure> {
    if !res.ok() {
        return Err(Failure::Provider(ProviderError::from_response(res)));
    }
    let Some(payload) = res.json() else {
        return Err(Failure::Final(Error::new(
            "the model's provider sent an unreadable reply",
            "INVALID_RESPONSE",
            502,
        )));
    };
    // A `200 OK` is not a promise: it can carry an `error` instead of a completion.
    if let Some(p) = ProviderError::in_body(&payload) {
        return Err(Failure::Provider(p));
    }
    if payload
        .pointer("/choices/0/message")
        .is_none_or(|m| !m.is_object())
    {
        return Err(Failure::Final(Error::new(
            "the model's provider's reply has no completion",
            "INVALID_RESPONSE",
            502,
        )));
    }
    if let Some(e) = empty_completion(&payload, model, max_tokens) {
        return Err(Failure::Final(e));
    }
    Ok(payload)
}

#[cfg(test)]
mod tests;
