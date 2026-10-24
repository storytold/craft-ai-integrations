//! The provider end to end against a scripted account service and a scripted LLM provider: the
//! Craft AI layer's conformance checks, streaming, tool calls, structured output, cancelling,
//! errors, and where each key is allowed to go. No network.

use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use craft_ai_core::conformance::{
    run_cancel_paced, run_lifecycle_paced, run_rejects, run_unknown_job,
};
use craft_ai_core::job::{Event, JobState};
use craft_ai_core::{
    AiError, CapabilityId, ErrorKind, InputRole, ModelId, Payload, Provider, Request, SecretString,
};
use craft_ai_fantastico::{FantasticoConfig, FantasticoProvider, Wire, WireResponse};
use fantastico_sdk::{HttpRequest, Method};
use serde_json::{Value, json};

const ACCOUNT_KEY: &str = "ck-fant-account-needle";
const RUNTIME_KEY: &str = "rk-runtime-needle";
const BASE: &str = "https://fantastico.cc/api";
/// The model call, wherever the SDK sends it: which provider serves it is the account's business.
const CHAT: &str = "/chat/completions";

#[derive(Clone, Copy, PartialEq)]
enum Chat {
    Text,
    Tools,
    Json,
    /// A stream that takes a long time, for cancelling.
    Slow,
}

struct Fake {
    chat: Mutex<Chat>,
    mint_refused: bool,
    mints: AtomicUsize,
    /// (url, authorization header) of every request.
    log: Mutex<Vec<(String, String)>>,
}

impl Fake {
    fn new(chat: Chat) -> Arc<Self> {
        Arc::new(Self {
            chat: Mutex::new(chat),
            mint_refused: false,
            mints: AtomicUsize::new(0),
            log: Mutex::new(Vec::new()),
        })
    }

    fn refusing_mints() -> Arc<Self> {
        Arc::new(Self {
            mint_refused: true,
            ..Arc::into_inner(Self::new(Chat::Text)).unwrap()
        })
    }
}

fn reply(status: u16, content_type: &str, body: impl Read + Send + 'static) -> WireResponse {
    WireResponse {
        status,
        headers: vec![("Content-Type".into(), content_type.into())],
        body: Box::new(body),
    }
}

fn json_reply(status: u16, v: Value) -> WireResponse {
    reply(
        status,
        "application/json",
        std::io::Cursor::new(v.to_string()),
    )
}

fn sse(chunks: &[Value]) -> String {
    let mut s = String::from(": keep-alive\n\n");
    for c in chunks {
        s.push_str(&format!("data: {c}\n\n"));
    }
    s + "data: [DONE]\n\n"
}

/// One event line every 20 ms, for ten seconds: long enough to be cancelled mid-stream.
struct Slow(usize);

impl Read for Slow {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.0 >= 500 {
            return Ok(0);
        }
        self.0 += 1;
        std::thread::sleep(Duration::from_millis(20));
        let line = b"data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n";
        let n = line.len().min(buf.len());
        buf[..n].copy_from_slice(&line[..n]);
        Ok(n)
    }
}

impl Wire for Fake {
    fn send(&self, req: &HttpRequest) -> Result<WireResponse, String> {
        let auth = req.header("authorization").unwrap_or_default().to_owned();
        self.log.lock().unwrap().push((req.url.clone(), auth));
        if req.url == format!("{BASE}/edge/key") && req.method == Method::Post {
            if self.mint_refused {
                return Ok(json_reply(
                    402,
                    json!({"message": "No credit left", "type": "ALLOWANCE_EXHAUSTED"}),
                ));
            }
            self.mints.fetch_add(1, Ordering::SeqCst);
            return Ok(json_reply(
                200,
                json!({"runtimeKey": RUNTIME_KEY, "expiresAt": "2099-01-01T00:00:00.000Z",
                       "baseUrl": "https://models.example/v1",
                       "recordId": "rec-1", "limit": 5.0, "cost": "0"}),
            ));
        }
        if req.url == format!("{BASE}/edge/models") {
            return Ok(json_reply(
                200,
                json!({"models": [
                    {"id": "anthropic/claude-sonnet-5", "name": "Claude Sonnet 5",
                     "architecture": {"output_modalities": ["text"]},
                     "supported_parameters": ["tools", "response_format"],
                     "pricing": {"prompt": "0.000003", "completion": "0.000015"}},
                    {"id": "black-forest-labs/flux", "architecture": {"output_modalities": ["image"]}}
                ]}),
            ));
        }
        if req.url.starts_with(BASE) || !req.url.ends_with(CHAT) {
            return Ok(json_reply(404, json!({"message": "no such route"})));
        }
        let body: Value = serde_json::from_str(req.body.as_deref().unwrap_or("{}")).unwrap();
        let stream = body["stream"] == true;
        let mode = *self.chat.lock().unwrap();
        let usage = json!({"prompt_tokens": 3, "completion_tokens": 4, "cost": 0.00042});
        Ok(match (mode, stream) {
            (Chat::Slow, _) => reply(200, "text/event-stream", Slow(0)),
            (Chat::Text, true) => reply(
                200,
                "text/event-stream",
                std::io::Cursor::new(sse(&[
                    json!({"id": "g", "choices": [{"delta": {"role": "assistant", "content": "Hel"}}]}),
                    json!({"choices": [{"delta": {"content": "lo"}}]}),
                    json!({"choices": [{"delta": {}, "finish_reason": "stop"}], "usage": usage}),
                ])),
            ),
            (Chat::Text, false) => json_reply(
                200,
                json!({"choices": [{"message": {"role": "assistant", "content": "Hello"},
                        "finish_reason": "stop"}], "usage": usage}),
            ),
            (Chat::Tools, _) => json_reply(
                200,
                json!({"choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "c1", "type": "function",
                     "function": {"name": "type_text", "arguments": "{\"text\":\"Hi\"}"}}]},
                    "finish_reason": "tool_calls"}]}),
            ),
            (Chat::Json, true) => reply(
                200,
                "text/event-stream",
                std::io::Cursor::new(sse(&[
                    json!({"choices": [{"delta": {"content": "{\"a\":"}}]}),
                    json!({"choices": [{"delta": {"content": " 1}"}, "finish_reason": "stop"}]}),
                ])),
            ),
            (Chat::Json, false) => json_reply(
                200,
                json!({"choices": [{"message": {"content": "{\"a\": 1}"}, "finish_reason": "stop"}]}),
            ),
        })
    }
}

fn provider(fake: &Arc<Fake>) -> FantasticoProvider {
    let config = FantasticoConfig::new(SecretString::new(ACCOUNT_KEY));
    FantasticoProvider::with_wire(config, Arc::clone(fake) as Arc<dyn Wire>).unwrap()
}

fn model() -> ModelId {
    ModelId::new("anthropic/claude-sonnet-5").unwrap()
}

fn chat(prompt: &str) -> Request {
    Request::new(CapabilityId::parse("text.complete").unwrap())
        .with_input(InputRole::Prompt, Payload::text(prompt))
}

fn pause() {
    std::thread::sleep(Duration::from_millis(5));
}

/// Runs a job to its end and returns the end state, every event and the usage.
fn finish(p: &FantasticoProvider, req: &Request) -> (JobState, Vec<Event>, Option<u64>) {
    let job = p.submit(&model(), req).unwrap();
    let mut events = Vec::new();
    for _ in 0..2000 {
        let snap = p.poll(&job).unwrap();
        events.extend(snap.events);
        if snap.state.is_terminal() {
            return (snap.state, events, snap.usage.and_then(|u| u.cost_micros));
        }
        pause();
    }
    panic!("job did not finish");
}

fn texts(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::TextDelta { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn passes_the_layers_conformance_checks() {
    let fake = Fake::new(Chat::Text);
    let p = provider(&fake);
    let done = run_lifecycle_paced(&p, &model(), &chat("hi"), &pause).unwrap();
    assert_eq!(done.state, JobState::Succeeded);
    run_unknown_job(&p).unwrap();
    let mask = chat("hi").with_input(InputRole::Mask, Payload::text("m"));
    run_rejects(&p, &model(), &mask, ErrorKind::Unsupported).unwrap();
    let image = Request::new(CapabilityId::parse("image.generate").unwrap())
        .with_input(InputRole::Prompt, Payload::text("a cat"));
    run_rejects(&p, &model(), &image, ErrorKind::Unsupported).unwrap();
    let empty = Request::new(CapabilityId::parse("text.complete").unwrap());
    run_rejects(&p, &model(), &empty, ErrorKind::InvalidRequest).unwrap();

    *fake.chat.lock().unwrap() = Chat::Slow;
    run_cancel_paced(&p, &model(), &chat("hi").streaming(), &pause).unwrap();
}

#[test]
fn a_whole_reply_arrives_as_one_text_with_its_cost() {
    let p = provider(&Fake::new(Chat::Text));
    let (state, events, cost) = finish(&p, &chat("hi"));
    assert_eq!(state, JobState::Succeeded);
    assert_eq!(texts(&events), ["Hello"]);
    assert_eq!(cost, Some(420));
}

#[test]
fn a_streamed_reply_arrives_in_pieces_and_is_not_repeated_at_the_end() {
    let p = provider(&Fake::new(Chat::Text));
    let (state, events, cost) = finish(&p, &chat("hi").streaming());
    assert_eq!(state, JobState::Succeeded);
    assert_eq!(texts(&events), ["Hel", "lo"]);
    assert_eq!(cost, Some(420));
}

#[test]
fn tool_calls_become_tool_call_events() {
    let p = provider(&Fake::new(Chat::Tools));
    let req = chat("say hi").with_param("tools", json!([{"type": "function"}]));
    let (state, events, _) = finish(&p, &req);
    assert_eq!(state, JobState::Succeeded);
    assert_eq!(
        events,
        [Event::ToolCall {
            name: "type_text".into(),
            arguments: "{\"text\":\"Hi\"}".into()
        }]
    );
}

#[test]
fn structured_output_is_one_json_output_streamed_or_not() {
    let p = provider(&Fake::new(Chat::Json));
    for req in [
        chat("json").with_param("response_format", json!({"type": "json_object"})),
        chat("json")
            .with_param("response_format", json!({"type": "json_object"}))
            .streaming(),
    ] {
        let (state, events, _) = finish(&p, &req);
        assert_eq!(state, JobState::Succeeded);
        assert!(texts(&events).is_empty(), "no raw JSON pieces");
        let [Event::Output(o)] = events.as_slice() else {
            panic!("{events:?}")
        };
        assert_eq!(
            o.payload,
            Payload::inline("application/json", b"{\"a\":1}".to_vec()).unwrap()
        );
    }
}

#[test]
fn models_come_from_the_account_and_non_chat_models_are_left_out() {
    let p = provider(&Fake::new(Chat::Text));
    let models = p.models().unwrap();
    let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["anthropic/claude-sonnet-5"]);
    assert!(models[0].pricing.is_some());
}

#[test]
fn each_key_goes_only_where_it_belongs_and_one_key_serves_many_jobs() {
    let fake = Fake::new(Chat::Text);
    let p = provider(&fake);
    for _ in 0..3 {
        assert_eq!(finish(&p, &chat("hi")).0, JobState::Succeeded);
    }
    assert_eq!(
        fake.mints.load(Ordering::SeqCst),
        1,
        "the runtime key is reused"
    );
    for (url, auth) in fake.log.lock().unwrap().iter() {
        if url.starts_with(BASE) {
            assert_eq!(auth, &format!("Bearer {ACCOUNT_KEY}"), "{url}");
        } else {
            // The runtime key goes where the mint said, and nowhere else.
            assert_eq!(url, &format!("https://models.example/v1{CHAT}"));
            assert_eq!(auth, &format!("Bearer {RUNTIME_KEY}"));
        }
    }
}

#[test]
fn an_empty_account_is_a_quota_error_without_the_key_in_it() {
    let p = provider(&Fake::refusing_mints());
    let (state, events, _) = finish(&p, &chat("hi"));
    let JobState::Failed { error } = state else {
        panic!("{state:?}")
    };
    let AiError { kind, message, .. } = error;
    assert_eq!(kind, ErrorKind::Quota);
    assert!(!message.contains("needle"));
    assert!(events.is_empty());
}

#[test]
fn too_many_jobs_at_once_is_a_rate_limit_not_a_queue() {
    let p = provider(&Fake::new(Chat::Slow));
    let a = p.submit(&model(), &chat("a").streaming()).unwrap();
    let b = p.submit(&model(), &chat("b").streaming()).unwrap();
    let third = p.submit(&model(), &chat("c").streaming()).unwrap_err();
    assert_eq!(third.kind, ErrorKind::RateLimited);
    p.cancel(&a).unwrap();
    p.cancel(&b).unwrap();
    assert!(p.submit(&model(), &chat("d")).is_ok());
}

/// What an agent loop does: the conversation so far as turns, the whole reply back.
#[test]
fn a_conversation_goes_out_as_turns_and_the_whole_reply_comes_back() {
    let p = provider(&Fake::new(Chat::Tools));
    let turn = |v: Value| {
        (
            InputRole::parse(craft_ai_fantastico::TURN).unwrap(),
            Payload::text(v.to_string()),
        )
    };
    let mut req = Request::new(CapabilityId::parse("text.complete").unwrap())
        .with_param("tools", json!([{"type": "function"}]))
        .streaming();
    for (role, payload) in [
        turn(json!({"role": "system", "content": "You edit documents."})),
        turn(json!({"role": "user", "content": "type hi"})),
    ] {
        req = req.with_input(role, payload);
    }
    let (state, events, _) = finish(&p, &req);
    assert_eq!(state, JobState::Succeeded);
    let completion = events
        .iter()
        .find_map(|e| match e {
            Event::Output(o) if o.label.as_deref() == Some(craft_ai_fantastico::COMPLETION) => {
                Some(o.payload.clone())
            }
            _ => None,
        })
        .unwrap();
    let Payload::Inline { data, .. } = completion else {
        panic!()
    };
    let reply: Value = serde_json::from_slice(&data).unwrap();
    assert_eq!(reply["choices"][0]["message"]["tool_calls"][0]["id"], "c1");
    assert_eq!(reply["choices"][0]["finish_reason"], "tool_calls");
}
