//! The account client against a scripted wire: each request takes the next queued reply and must go
//! to the expected address. No network.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::*;
use crate::time::format_iso_ms;
use crate::transport::{Wire, WireResponse};

type Log = Arc<Mutex<Vec<HttpRequest>>>;

struct Fake {
    queue: Mutex<VecDeque<(&'static str, HttpResponse)>>,
    log: Log,
}

impl Wire for Fake {
    fn send(&self, req: &HttpRequest) -> Result<WireResponse, String> {
        // The price list (for `max_tokens`) is asked for when the key has a cap: unless the test
        // queued one, it is empty and not counted among the requests.
        let listed = self
            .queue
            .lock()
            .unwrap()
            .front()
            .is_some_and(|(w, _)| w.ends_with("/models"));
        let res = if req.url.ends_with("/models") && !listed {
            res(200, json!({"models": []}))
        } else {
            self.log.lock().unwrap().push(req.clone());
            let (want, res) = self
                .queue
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected request: {req:?}"));
            if want == "NETWORK" {
                return Err("connection refused".into());
            }
            assert!(
                req.url.ends_with(want),
                "expected `{want}`, got {}",
                req.url
            );
            res
        };
        Ok(WireResponse {
            status: res.status,
            headers: res.headers,
            body: Box::new(std::io::Cursor::new(res.body.into_bytes())),
        })
    }
}

fn res(status: u16, body: Value) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![],
        body: body.to_string(),
    }
}

/// The address every fixture key is accepted at. A made-up host: the request must go wherever the
/// mint says, not to an address of the adapter's own.
const KEY_BASE: &str = "https://llm.example.test/v1";

fn mint_at(limit: f64, record: &str, base_url: &str) -> HttpResponse {
    res(
        200,
        json!({"runtimeKey": format!("rk-{record}"), "expiresAt": format_iso_ms(now_ms() + 3_600_000),
               "recordId": record, "limit": limit, "activitySessionId": "sess-1",
               "baseUrl": base_url, "defaultModel": "vendor/model-default",
               "chatDefaults": {"usage": {"include": true}}}),
    )
}

fn mint(limit: f64, record: &str) -> HttpResponse {
    mint_at(limit, record, KEY_BASE)
}

fn minted(fields: Value) -> HttpResponse {
    let mut v = json!({"runtimeKey": "rk-1", "expiresAt": format_iso_ms(now_ms() + 3_600_000),
                       "baseUrl": KEY_BASE, "defaultModel": "m"});
    if let (Some(v), Some(f)) = (v.as_object_mut(), fields.as_object()) {
        v.extend(f.clone());
    }
    res(200, v)
}

fn completion(text: &str) -> HttpResponse {
    res(
        200,
        json!({"model": "m", "choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]}),
    )
}

fn provider_err(status: u16, error_type: Option<&str>) -> HttpResponse {
    let metadata = error_type.map_or(Value::Null, |t| json!({"error_type": t}));
    res(
        status,
        json!({"error": {"code": status, "message": format!("refused {status}"), "metadata": metadata}}),
    )
}

fn options() -> Options {
    Options {
        base_url: "https://fantastico.example.test/api/".into(),
        product: None,
        title: "Craft".into(),
        budget: Duration::from_secs(90),
    }
}

fn client_with(
    key: &str,
    opts: Options,
    script: Vec<(&'static str, HttpResponse)>,
) -> (Account, Log) {
    let log: Log = Arc::default();
    let fake = Fake {
        queue: Mutex::new(script.into()),
        log: Arc::clone(&log),
    };
    let account = Account::new(SecretString::new(key), opts, Transport::new(Arc::new(fake)))
        .unwrap()
        .with_sleep(|_| {});
    (account, log)
}

fn client(script: Vec<(&'static str, HttpResponse)>) -> (Account, Log) {
    client_with("ck-fant-abc", options(), script)
}

fn ask() -> Value {
    json!({"messages": [{"role": "user", "content": "hello"}]})
}

fn body(r: &HttpRequest) -> Value {
    serde_json::from_str(r.body.as_deref().unwrap_or("{}")).unwrap()
}

fn content(reply: &Value) -> &str {
    reply["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
}

#[test]
fn the_prefix_chooses_the_surface() {
    let (c, _) = client(vec![]);
    assert_eq!(c.surface, Surface::Edge);
    let (c, log) = client_with("  sk-fant-prod-abc ", options(), vec![]);
    assert_eq!(c.surface, Surface::Runtime);
    // A project key has many accounts: reading one is refused before anything is sent.
    assert_eq!(c.info().unwrap_err().code, "SURFACE_UNSUPPORTED");
    assert_eq!(c.balance().unwrap_err().code, "SURFACE_UNSUPPORTED");
    assert!(log.lock().unwrap().is_empty());
    for bad in ["", "rk-123", "fant-abc", "CK-FANT-abc"] {
        let fake = Fake {
            queue: Mutex::default(),
            log: Arc::default(),
        };
        let e = Account::new(
            SecretString::new(bad),
            options(),
            Transport::new(Arc::new(fake)),
        )
        .err()
        .unwrap();
        assert_eq!(e.code, "UNKNOWN_CREDENTIAL", "{bad}");
    }
}

#[test]
fn mints_once_and_each_key_goes_only_where_it_belongs() {
    let (mut c, log) = client(vec![
        ("/edge/key", mint(5.0, "r1")),
        ("/chat/completions", completion("uno")),
        ("/chat/completions", completion("due")),
    ]);
    assert_eq!(content(&c.chat_completion(&ask()).unwrap()), "uno");
    assert_eq!(content(&c.chat_completion(&ask()).unwrap()), "due");
    let log = log.lock().unwrap();
    assert_eq!(log.len(), 3);
    // The account service sees the account key, the provider the runtime key; never the reverse.
    assert_eq!(log[0].url, "https://fantastico.example.test/api/edge/key");
    assert_eq!(log[0].method, Method::Post);
    assert_eq!(log[0].header("Authorization"), Some("Bearer ck-fant-abc"));
    assert_eq!(log[1].header("Authorization"), Some("Bearer rk-r1"));
    assert_eq!(log[1].header("X-Title"), Some("Craft"));
    assert_eq!(log[1].header("HTTP-Referer"), Some(REFERER));
    assert_eq!(log[1].url, format!("{KEY_BASE}/chat/completions"));
    let sent = body(&log[1]);
    assert_eq!(sent["model"], "vendor/model-default");
    // Asked for by the mint, not written by the adapter.
    assert_eq!(sent["usage"], json!({"include": true}));
    // A roomy cap (5 USD): no max_tokens is written on the caller's behalf.
    assert!(sent.get("max_tokens").is_none());
    // Neither key shows when a request is printed.
    let shown = format!("{log:?}");
    assert!(
        !shown.contains("ck-fant-abc") && !shown.contains("rk-r1"),
        "{shown}"
    );
}

#[test]
fn the_session_and_product_go_to_every_mint() {
    let opts = Options {
        product: Some("example-product".into()),
        ..options()
    };
    let (mut c, log) = client_with(
        "ck-fant-abc",
        opts,
        vec![
            ("/edge/key", mint(5.0, "r1")),
            ("/chat/completions", provider_err(401, None)),
            ("/edge/key", mint(5.0, "r2")),
            ("/chat/completions", completion("ok")),
        ],
    );
    c.chat_completion(&ask()).unwrap();
    let log = log.lock().unwrap();
    assert_eq!(body(&log[0]), json!({"product": "example-product"}));
    assert_eq!(
        body(&log[2]),
        json!({"product": "example-product", "activitySessionId": "sess-1"})
    );
}

#[test]
fn an_expiring_key_is_replaced() {
    let soon =
        minted(json!({"runtimeKey": "rk-old", "expiresAt": format_iso_ms(now_ms() + 30_000)}));
    let (mut c, log) = client(vec![
        ("/edge/key", soon),
        ("/chat/completions", completion("a")),
        ("/edge/key", mint(5.0, "new")),
        ("/chat/completions", completion("b")),
    ]);
    c.chat_completion(&ask()).unwrap();
    c.chat_completion(&ask()).unwrap();
    assert_eq!(
        log.lock().unwrap()[3].header("Authorization"),
        Some("Bearer rk-new")
    );
}

#[test]
fn a_rejected_key_is_reminted_once_and_spent_at_its_own_address() {
    let (mut c, log) = client(vec![
        (
            "/edge/key",
            mint_at(0.5, "r1", "https://one.example.test/v1"),
        ),
        (
            "/chat/completions",
            provider_err(402, Some("payment_required")),
        ),
        (
            "/edge/key",
            mint_at(0.5, "r2", "https://two.example.test/v1/"),
        ),
        ("/chat/completions", completion("ok")),
    ]);
    assert_eq!(content(&c.chat_completion(&ask()).unwrap()), "ok");
    let log = log.lock().unwrap();
    assert_eq!(log[1].url, "https://one.example.test/v1/chat/completions");
    // A trailing slash on the mint's address is dropped, not doubled.
    assert_eq!(log[3].url, "https://two.example.test/v1/chat/completions");
    assert_eq!(log[3].header("Authorization"), Some("Bearer rk-r2"));

    // Twice in a row: a provider error, not a verdict on the account's funds.
    let (mut c, log) = client(vec![
        ("/edge/key", mint(0.5, "r1")),
        ("/chat/completions", provider_err(402, None)),
        ("/edge/key", mint(0.5, "r2")),
        ("/chat/completions", provider_err(402, None)),
    ]);
    let e = c.chat_completion(&ask()).unwrap_err();
    assert_eq!((e.code.as_str(), e.status), ("LLM_PROVIDER_ERROR", 502));
    assert_eq!(log.lock().unwrap().len(), 4);

    // The second attempt failing for a reason the provider named keeps that name.
    let (mut c, _) = client(vec![
        ("/edge/key", mint(0.5, "r1")),
        ("/chat/completions", provider_err(401, None)),
        ("/edge/key", mint(0.5, "r2")),
        (
            "/chat/completions",
            provider_err(400, Some("context_length_exceeded")),
        ),
    ]);
    assert_eq!(
        c.chat_completion(&ask()).unwrap_err().code,
        "PROVIDER_CONTEXT_LENGTH_EXCEEDED"
    );
}

#[test]
fn an_error_inside_a_200_triggers_the_remint() {
    let embedded = res(
        200,
        json!({"error": {"code": 402, "message": "no credits", "metadata": {"error_type": "payment_required"}}}),
    );
    let (mut c, _) = client(vec![
        ("/edge/key", mint(5.0, "r1")),
        ("/chat/completions", embedded),
        ("/edge/key", mint(5.0, "r2")),
        ("/chat/completions", completion("ok")),
    ]);
    assert!(c.chat_completion(&ask()).is_ok());
}

#[test]
fn a_refused_mint_is_the_account_services_verdict() {
    let refused = res(
        402,
        json!({"message": "Credit exhausted", "type": "ALLOWANCE_EXHAUSTED", "code": 402}),
    );
    let (mut c, log) = client(vec![("/edge/key", refused)]);
    assert_eq!(
        c.chat_completion(&ask()).unwrap_err(),
        Error::new("Credit exhausted", "ALLOWANCE_EXHAUSTED", 402)
    );
    assert_eq!(log.lock().unwrap().len(), 1);
    // Without a message or a type, the status and a code of the adapter's own.
    let (mut c, _) = client(vec![("/edge/key", res(500, json!({})))]);
    let e = c.chat_completion(&ask()).unwrap_err();
    assert_eq!((e.code.as_str(), e.status), ("MINT_FAILED", 500));
    assert!(e.message.contains("500"), "{}", e.message);
}

#[test]
fn moderation_is_not_reminted_and_keeps_its_name() {
    let (mut c, log) = client(vec![
        ("/edge/key", mint(5.0, "r1")),
        (
            "/chat/completions",
            provider_err(403, Some("content_policy_violation")),
        ),
    ]);
    let e = c.chat_completion(&ask()).unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.status),
        ("PROVIDER_CONTENT_POLICY_VIOLATION", 403)
    );
    assert_eq!(log.lock().unwrap().len(), 2);
}

#[test]
fn rate_limits_are_retried_with_backoff_then_named() {
    let mut limited = provider_err(429, Some("rate_limit_exceeded"));
    limited.headers.push(("retry-after".into(), "2".into()));
    let slept: Arc<Mutex<Vec<Duration>>> = Arc::default();
    let s = Arc::clone(&slept);
    let (c, log) = client(vec![
        ("/edge/key", mint(5.0, "r1")),
        ("/chat/completions", limited.clone()),
        ("/chat/completions", limited),
        ("/chat/completions", completion("ok")),
    ]);
    let mut c = c.with_sleep(move |d| s.lock().unwrap().push(d));
    assert!(c.chat_completion(&ask()).is_ok());
    let slept = slept.lock().unwrap();
    assert_eq!(slept.len(), 2);
    // Between half and all of Retry-After, never less.
    assert!(
        slept.iter().all(|d| (1000..=2000).contains(&d.as_millis())),
        "{slept:?}"
    );
    // The same request, with the same key: a retry is not a new key.
    assert!(
        log.lock().unwrap()[1..]
            .iter()
            .all(|r| r.header("Authorization") == Some("Bearer rk-r1"))
    );

    // Once the retries are used up, the 429 arrives under its own name, and no key is spent on it.
    let l = provider_err(429, Some("rate_limit_exceeded"));
    let (mut c, log) = client(vec![
        ("/edge/key", mint(5.0, "r1")),
        ("/chat/completions", l.clone()),
        ("/chat/completions", l.clone()),
        ("/chat/completions", l),
    ]);
    assert_eq!(
        c.chat_completion(&ask()).unwrap_err().code,
        "PROVIDER_RATE_LIMIT_EXCEEDED"
    );
    assert_eq!(log.lock().unwrap().len(), 4);

    // A 502 may have been answered and lost on the way back: never sent again.
    let (mut c, log) = client(vec![
        ("/edge/key", mint(5.0, "r1")),
        ("/chat/completions", provider_err(502, None)),
    ]);
    assert_eq!(
        c.chat_completion(&ask()).unwrap_err().code,
        "INTERNAL_ERROR"
    );
    assert_eq!(log.lock().unwrap().len(), 2);
}

#[test]
fn the_wait_doubles_is_capped_and_jittered_over_its_upper_half() {
    let p = |after: Option<u64>| ProviderError {
        message: String::new(),
        http_status: 429,
        provider_code: None,
        error_type: None,
        retry_after_ms: after,
    };
    let ms = |d: Duration| d.as_millis();
    assert_eq!(ms(delay_for(&p(None), 1, 0.0)), 250);
    assert_eq!(ms(delay_for(&p(None), 1, 0.999_999)), 500);
    assert_eq!(ms(delay_for(&p(None), 2, 0.999_999)), 1000);
    assert_eq!(ms(delay_for(&p(None), 30, 0.999_999)), 10_000);
    assert_eq!(ms(delay_for(&p(Some(4000)), 1, 0.0)), 2000);
    assert_eq!(ms(delay_for(&p(Some(60_000)), 1, 0.999_999)), 10_000);
    assert_eq!(ms(delay_for(&p(None), 1, 7.0)), 500);
}

#[test]
fn the_budget_bounds_each_request_and_the_waits() {
    let opts = Options {
        budget: Duration::from_secs(5),
        ..options()
    };
    let (mut c, log) = client_with(
        "ck-fant-abc",
        opts,
        vec![
            ("/edge/key", mint(5.0, "r1")),
            ("/chat/completions", completion("ok")),
        ],
    );
    c.chat_completion(&ask()).unwrap();
    let log = log.lock().unwrap();
    assert!(
        log[1].timeout <= Duration::from_secs(5),
        "{:?}",
        log[1].timeout
    );
    // The account service has its own time, not the provider's budget.
    assert_eq!(log[0].timeout, CLOUD_TIMEOUT);

    // A spent budget sends nothing to the provider: the key is minted, the request isn't.
    let opts = Options {
        budget: Duration::ZERO,
        ..options()
    };
    let (mut c, log) = client_with("ck-fant-abc", opts, vec![("/edge/key", mint(5.0, "r1"))]);
    assert_eq!(c.chat_completion(&ask()).unwrap_err().code, "TIMEOUT");
    assert_eq!(log.lock().unwrap().len(), 1);

    // A wait that would end past the budget is not slept: the rate limit is reported instead.
    let mut long = provider_err(429, None);
    long.headers.push(("Retry-After".into(), "9".into()));
    let opts = Options {
        budget: Duration::from_secs(3),
        ..options()
    };
    let (c, _) = client_with(
        "ck-fant-abc",
        opts,
        vec![("/edge/key", mint(5.0, "r1")), ("/chat/completions", long)],
    );
    let mut c = c.with_sleep(|_| panic!("slept past the budget"));
    assert_eq!(
        c.chat_completion(&ask()).unwrap_err().code,
        "INTERNAL_ERROR"
    );
}

#[test]
fn an_empty_completion_at_the_limit_is_an_error() {
    let empty = res(
        200,
        json!({"choices": [{"message": {"content": ""}, "finish_reason": "length"}], "usage": {"completion_tokens": 64}}),
    );
    let (mut c, _) = client(vec![
        ("/edge/key", mint(5.0, "r1")),
        ("/chat/completions", empty),
    ]);
    assert_eq!(
        c.chat_completion(&ask()).unwrap_err().code,
        "EMPTY_COMPLETION"
    );
}

#[test]
fn max_tokens_follows_the_key_limit_unless_the_request_sets_it() {
    let models = res(
        200,
        json!({"models": [{"id": "vendor/model-default", "pricing": {"completion": "0.000015"}}]}),
    );
    let (mut c, log) = client(vec![
        ("/edge/key", mint(0.05, "r1")),
        ("/edge/models", models),
        ("/chat/completions", completion("ok")),
        ("/chat/completions", completion("ok")),
    ]);
    c.chat_completion(&ask()).unwrap();
    // 0.05 * 0.8 / 0.000015 = 2666
    assert_eq!(body(&log.lock().unwrap()[2])["max_tokens"], 2666);
    // An explicit max_tokens passes untouched (and the price list stays cached).
    c.chat_completion(&json!({"messages": [], "max_tokens": 10}))
        .unwrap();
    assert_eq!(body(&log.lock().unwrap()[3])["max_tokens"], 10);

    assert_eq!(affordable(Some(100.0), Some(0.000015)), None);
    assert_eq!(affordable(Some(0.0001), Some(0.000015)), Some(64));
    for (l, p) in [
        (None, Some(1.0)),
        (Some(1.0), None),
        (Some(f64::NAN), Some(1.0)),
        (Some(1.0), Some(f64::INFINITY)),
        (Some(-1.0), Some(1.0)),
        (Some(1.0), Some(0.0)),
    ] {
        assert_eq!(affordable(l, p), None, "{l:?} {p:?}");
    }
}

#[test]
fn chat_defaults_are_merged_under_what_the_request_writes() {
    let key = minted(
        json!({"chatDefaults": {"usage": {"include": true}, "temperature": 0.1,
                                             "model": "vendor/from-defaults", "max_tokens": 1}}),
    );
    let (mut c, log) = client(vec![
        ("/edge/key", key),
        ("/chat/completions", completion("ok")),
    ]);
    c.chat_completion(&json!({"messages": [], "temperature": 0.7, "max_tokens": 50}))
        .unwrap();
    let sent = body(&log.lock().unwrap()[1]);
    // The mint's own fields arrive untouched…
    assert_eq!(sent["usage"], json!({"include": true}));
    // …and every field the request (or the adapter, for the model) writes wins over them.
    assert_eq!(sent["temperature"], 0.7);
    assert_eq!(sent["max_tokens"], 50);
    assert_eq!(sent["model"], "m");

    // Without defaults, nothing is added.
    let (mut c, log) = client(vec![
        ("/edge/key", minted(json!({}))),
        ("/chat/completions", completion("ok")),
    ]);
    c.chat_completion(&ask()).unwrap();
    assert!(body(&log.lock().unwrap()[1]).get("usage").is_none());
}

#[test]
fn the_model_comes_from_the_request_then_the_mint() {
    let (mut c, log) = client(vec![
        ("/edge/key", mint(5.0, "r1")),
        ("/chat/completions", completion("ok")),
    ]);
    c.chat_completion(&json!({"model": "vendor/explicit", "messages": []}))
        .unwrap();
    assert_eq!(body(&log.lock().unwrap()[1])["model"], "vendor/explicit");

    // Nobody names one: refused before reaching the provider.
    let (mut c, log) = client(vec![("/edge/key", minted(json!({"defaultModel": null})))]);
    let e = c
        .chat_completion(&json!({"model": "", "messages": []}))
        .unwrap_err();
    assert_eq!((e.code.as_str(), e.status), ("MODEL_REQUIRED", 400));
    assert_eq!(log.lock().unwrap().len(), 1);
}

#[test]
fn a_key_without_an_address_is_refused_with_no_fallback() {
    for base in [Value::Null, json!(""), json!("/"), json!(7)] {
        let (mut c, log) = client(vec![("/edge/key", minted(json!({"baseUrl": base})))]);
        let e = c.chat_completion(&ask()).unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.status),
            ("PROVIDER_ACCESS_MISSING", 502),
            "{base}"
        );
        // Nothing was sent anywhere after the mint, and no key is kept.
        assert_eq!(log.lock().unwrap().len(), 1);
        assert!(c.runtime.is_none());
    }
}

#[test]
fn hostile_replies_never_panic() {
    let garbage = [
        res(200, json!({})),
        res(
            200,
            json!({"runtimeKey": 7, "expiresAt": "yesterday", "baseUrl": KEY_BASE}),
        ),
        minted(json!({"expiresAt": "2026-13-45T99:99:99Z"})),
        HttpResponse {
            status: 200,
            headers: vec![],
            body: "<html>".into(),
        },
        HttpResponse {
            status: 500,
            headers: vec![],
            body: "x".repeat(10_000),
        },
        res(200, json!([1, 2, 3])),
    ];
    for g in garbage {
        let (mut c, _) = client(vec![("/edge/key", g)]);
        assert!(c.chat_completion(&ask()).is_err());
    }
    let hostile_completions = [
        res(200, json!({"choices": []})),
        res(200, json!({"choices": [{"message": null}]})),
        HttpResponse {
            status: 200,
            headers: vec![],
            body: "not json".into(),
        },
        res(200, json!({"error": "a string"})),
        HttpResponse {
            status: 503,
            headers: vec![("Retry-After".into(), "-5".into())],
            body: "{".into(),
        },
    ];
    for h in hostile_completions {
        let (mut c, _) = client(vec![
            ("/edge/key", mint(5.0, "r")),
            ("/chat/completions", h.clone()),
            ("/chat/completions", h.clone()),
            ("/chat/completions", h),
        ]);
        assert!(c.chat_completion(&ask()).is_err());
    }
    let (mut c, _) = client(vec![("NETWORK", res(0, json!({})))]);
    assert_eq!(c.chat_completion(&ask()).unwrap_err().code, "NETWORK_ERROR");
    let (mut c, _) = client(vec![
        ("/edge/key", mint(5.0, "r")),
        ("NETWORK", res(0, json!({}))),
    ]);
    assert_eq!(c.chat_completion(&ask()).unwrap_err().code, "NETWORK_ERROR");
    let (mut c, _) = client(vec![]);
    assert_eq!(
        c.chat_completion(&json!("text")).unwrap_err().code,
        "INVALID_REQUEST"
    );
}

#[test]
fn the_account_and_its_credit_are_read_without_minting() {
    let config = res(
        200,
        json!({"contoId": "conto-42", "status": "active", "model": "vendor/model-suggested",
               "balance": {"remainingUsd": 0.5, "lowBalance": false}}),
    );
    let balance = res(
        200,
        json!({"remainingUsd": "0.03", "lowBalance": true, "status": "active"}),
    );
    let (c, log) = client(vec![
        ("/edge/config", config),
        ("/edge/balance", balance),
        (
            "/edge/config",
            res(200, json!({"contoId": "c", "balance": null})),
        ),
        ("/edge/config", res(200, json!({"status": "active"}))),
        ("/edge/balance", res(200, json!({"lowBalance": true}))),
        (
            "/edge/config",
            res(401, json!({"message": "no", "type": "INVALID_CONTO_KEY"})),
        ),
    ]);
    let info = c.info().unwrap();
    assert_eq!(
        (
            info.account_id.as_str(),
            info.model.as_deref(),
            info.status.as_str()
        ),
        ("conto-42", Some("vendor/model-suggested"), "active")
    );
    assert_eq!(info.balance.unwrap().remaining_usd, 0.5);
    let b = c.balance().unwrap();
    assert!(b.low_balance && (b.remaining_usd - 0.03).abs() < 1e-12);
    // An unreadable balance doesn't stop the account from being read.
    let bare = c.info().unwrap();
    assert_eq!(
        (bare.balance, bare.status.as_str(), bare.model),
        (None, "active", None)
    );
    // Without an account id, or a credit, the reply is incomplete.
    assert_eq!(c.info().unwrap_err().code, "INVALID_RESPONSE");
    assert_eq!(c.balance().unwrap_err().code, "INVALID_RESPONSE");
    assert_eq!(c.info().unwrap_err().code, "INVALID_CONTO_KEY");
    let log = log.lock().unwrap();
    assert!(
        log.iter()
            .all(|r| r.method == Method::Get && r.body.is_none())
    );
    assert!(
        log.iter()
            .all(|r| r.header("Authorization") == Some("Bearer ck-fant-abc"))
    );
}

#[test]
fn the_credit_shows_the_servers_total_and_never_a_guessed_or_broken_one() {
    let data = json!({"remainingUsd": 5, "reservedUnspentUsd": "4.97", "totalRemainingUsd": "9.97",
        "lowBalance": true, "lowTotalBalance": false, "status": "open"});
    let (c, _) = client(vec![
        (
            "/edge/config",
            res(200, json!({"contoId": "a", "balance": data.clone()})),
        ),
        ("/edge/balance", res(200, data)),
        (
            "/edge/balance",
            res(200, json!({"remainingUsd": 5, "lowBalance": false})),
        ),
    ]);
    let balance = c.info().unwrap().balance.unwrap();
    assert_eq!(balance.remaining_usd, 5.0);
    assert_eq!(balance.reserved_unspent_usd, Some(4.97));
    assert_eq!(balance.total_remaining_usd, Some(9.97));
    assert_eq!(balance.low_total_balance, Some(false));
    assert_eq!(balance.status.as_deref(), Some("open"));
    assert_eq!(c.balance().unwrap(), balance);
    // An older server: no total is made up.
    let older = c.balance().unwrap();
    assert_eq!(
        (
            older.total_remaining_usd,
            older.reserved_unspent_usd,
            older.low_total_balance
        ),
        (None, None, None)
    );
    for value in [
        json!("NaN"),
        json!("inf"),
        Value::Null,
        json!({}),
        json!("not money"),
    ] {
        let (c, _) = client(vec![(
            "/edge/balance",
            res(200, json!({"remainingUsd": 5, "totalRemainingUsd": value})),
        )]);
        assert_eq!(c.balance().unwrap().total_remaining_usd, None);
    }
}

#[test]
fn the_price_list_keeps_only_well_formed_prices() {
    let models = res(
        200,
        json!({"models": [
            {"id": "a", "pricing": {"completion": 0.00002}},
            {"id": "b", "pricing": {"completion": "0"}},
            {"id": "c", "pricing": {}},
            {"pricing": {"completion": 1.0}},
            "junk"
        ]}),
    );
    let (mut c, _) = client(vec![("/edge/models", models)]);
    assert_eq!(c.output_price("a"), Some(0.00002));
    for m in ["b", "c", "missing"] {
        assert_eq!(c.output_price(m), None, "{m}");
    }
}

#[test]
fn the_jitter_stays_in_its_range() {
    let (mut c, _) = client(vec![]);
    for _ in 0..1000 {
        let r = c.random();
        assert!((0.0..1.0).contains(&r), "{r}");
    }
}
