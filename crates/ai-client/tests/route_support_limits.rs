//! What the route check must also see: how many of each input a listed model takes, the address a
//! built-in adapter will use, a plan carried to a client whose build refuses it, and which real job
//! counts as the last success. Each refusal comes before consent and before any key is read.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use craft_ai_client::{ProviderFactory, RouteSupport};
use craft_ai_core::model::InputRole;
use craft_ai_core::{ConsentLedger, ErrorKind, ModelId, Payload};
use craft_ai_native::HistoryFile;
use craft_ai_native::shared::SharedStore;
use serde_json::json;

#[path = "support/route_fixture.rs"]
mod route_fixture;
use route_fixture::*;

/// A saved spec may allow two prompts, but the listed model takes one, as `submit` would enforce.
#[test]
fn more_of_an_input_than_a_listed_model_takes_is_refused_at_plan() {
    let dir = tempfile::tempdir().unwrap();
    let cap = "image.generate";
    let shared = saved_at(
        &dir,
        "artcraft-omni",
        LOOPBACK,
        "nano_banana",
        cap,
        json!({"prompt": 2}),
    );
    let reads = Arc::new(AtomicUsize::new(0));
    let c = counted_client(shared, ProviderFactory::standard(), reads.clone());
    assert!(
        c.plan(&role_for(cap), request_for(cap)).is_ok(),
        "one prompt is fine"
    );
    let two = request_for(cap).with_input(InputRole::Prompt, Payload::text("and a sail"));
    let refused = c.plan(&role_for(cap), two).unwrap_err();
    assert_eq!(refused.kind, ErrorKind::Unsupported);
    assert_eq!(reads.load(Ordering::SeqCst), 0, "no key was read");
}

/// A request input the listed model refuses is refused at plan, and the saved choice stays.
#[test]
fn a_request_input_the_listed_model_refuses_is_refused_at_plan() {
    let dir = tempfile::tempdir().unwrap();
    let cap = "image.generate";
    let shared = saved_at(
        &dir,
        "artcraft-omni",
        LOOPBACK,
        "nano_banana",
        cap,
        json!({"prompt": 1, "mask": 1}),
    );
    let before = std::fs::read(shared.path()).unwrap();
    let c = counted_client(
        shared.clone(),
        ProviderFactory::standard(),
        Arc::new(AtomicUsize::new(0)),
    );
    let with_mask = request_for(cap).with_input(
        InputRole::Mask,
        Payload::inline("image/png", vec![2]).unwrap(),
    );
    let refused = c.plan(&role_for(cap), with_mask).unwrap_err();
    assert_eq!(refused.kind, ErrorKind::Unsupported);
    assert_eq!(std::fs::read(shared.path()).unwrap(), before);
}

/// Each built-in adapter with a fixed address refuses another one when it is built, so the check
/// says so first, and accepts its official address and a test server here.
#[test]
fn a_built_in_adapter_refuses_an_address_it_would_not_use() {
    let cases = [
        (
            "artcraft-omni",
            "nano_banana",
            "image.generate",
            "https://api.storyteller.ai",
        ),
        (
            "bfl",
            "flux-3-image",
            "image.generate",
            "https://api.bfl.ai/v1",
        ),
        (
            "seedance",
            "dreamina-seedance-2-0-260128",
            "video.generate",
            "https://ark.ap-southeast.bytepluses.com/api/v3",
        ),
        (
            "vertex-express",
            "gemini-2.5-flash-image",
            "image.generate",
            "https://aiplatform.googleapis.com/v1",
        ),
    ];
    for (provider, model, cap, official) in cases {
        for (endpoint, works) in [
            (official, true),
            (LOOPBACK, true),
            ("https://elsewhere.example.com/v1", false),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let shared = saved_at(&dir, provider, endpoint, model, cap, json!({"prompt": 1}));
            let c = counted_client(
                shared,
                ProviderFactory::standard(),
                Arc::new(AtomicUsize::new(0)),
            );
            let executable = c.route_facts(&role_for(cap)).executable;
            if works {
                assert_eq!(
                    executable,
                    RouteSupport::Supported,
                    "{provider} at {endpoint}"
                );
            } else {
                assert!(
                    matches!(executable, RouteSupport::Unsupported { .. }),
                    "{provider} at {endpoint}"
                );
            }
        }
    }
}

/// A plan made where the route is supported, then started by a client whose build refuses it,
/// is refused before the key is read or a provider is built.
#[test]
fn start_checks_the_route_again_before_reading_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let cap = "image.generate";
    let shared = saved_at(
        &dir,
        "custom-gate",
        LOOPBACK,
        "any",
        cap,
        json!({"prompt": 1}),
    );
    let mut sure = ProviderFactory::standard();
    sure.register("custom-gate", |_, _| Ok(Arc::new(Untouched)));
    sure.declare_support("custom-gate", |_| RouteSupport::Supported);
    let planner = counted_client(shared.clone(), sure, Arc::new(AtomicUsize::new(0)));
    let plan = planner.plan(&role_for(cap), request_for(cap)).unwrap();
    let mut ledger = ConsentLedger::default();
    ledger.grant(plan.disclosure());

    let mut refuses = ProviderFactory::standard();
    refuses.register("custom-gate", |_, _| {
        panic!("a refused route must not build a provider")
    });
    refuses.declare_support("custom-gate", |_| RouteSupport::Unsupported {
        reason: "Not this one.".to_owned(),
    });
    let reads = Arc::new(AtomicUsize::new(0));
    let starter = counted_client(shared, refuses, reads.clone());
    let refused = starter.start(&plan, &ledger).err().expect("refused");
    assert_eq!(refused.kind, ErrorKind::Unsupported);
    assert_eq!(refused.message, "Not this one.");
    assert_eq!(reads.load(Ordering::SeqCst), 0, "no key was read");
}

/// One start and end pair in the history file, as JSON lines.
fn run(id: &str, model: &str, destination: &str, ended_at: u64, done: bool) -> String {
    let outcome = if done {
        json!({"status": "done"})
    } else {
        json!({"status": "failed", "kind": "provider"})
    };
    let start = json!({
        "phase": "start", "v": 1, "run": id, "at_ms": ended_at - 10, "app": "photocraft",
        "role_id": "fill", "role_label": "Fill", "capability": "image.generate",
        "connection": "one", "connection_label": "One", "model": model,
        "locality": "local", "destination": destination
    });
    let end = json!({"phase": "end", "v": 1, "run": id, "at_ms": ended_at, "outcome": outcome});
    format!("{start}\n{end}\n")
}

/// The last success is the newest finished job for this connection, model and address: a later
/// failure, another model's success and a success at an older address do not count.
#[test]
fn the_last_success_is_the_newest_done_job_at_the_current_address() {
    let dir = tempfile::tempdir().unwrap();
    let cap = "image.generate";
    let shared = saved_at(
        &dir,
        "custom-quiet",
        LOOPBACK,
        "any",
        cap,
        json!({"prompt": 1}),
    );
    let mut factory = ProviderFactory::standard();
    factory.register("custom-quiet", |_, _| Ok(Arc::new(Untouched)));
    let log = HistoryFile::new(dir.path().join("jobs.jsonl"));
    let c = counted_client(shared.clone(), factory, Arc::new(AtomicUsize::new(0)))
        .with_history(log.clone());
    let here = c
        .plan(&role_for(cap), request_for(cap))
        .unwrap()
        .disclosure()
        .destination
        .clone();
    let lines = [
        run(
            "old-address",
            "any",
            "https://before.example.com",
            5_000,
            true,
        ),
        run("match", "any", &here, 1_000, true),
        run("later-failure", "any", &here, 2_000, false),
        run("other-model", "other", &here, 3_000, true),
    ]
    .concat();
    std::fs::write(log.path(), lines).unwrap();
    assert_eq!(c.route_facts(&role_for(cap)).last_success_ms, Some(1_000));
}

/// Choosing a model the adapter refuses leaves the saved file exactly as it was, byte for byte:
/// a refusal must not rewrite it.
#[test]
fn a_refused_choice_does_not_rewrite_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let cap = "image.generate";
    let shared = saved_at(
        &dir,
        "artcraft-omni",
        LOOPBACK,
        "not_a_real_model",
        cap,
        json!({"prompt": 1}),
    );
    let before = std::fs::read(shared.path()).unwrap();
    let c = counted_client(
        shared.clone(),
        ProviderFactory::standard(),
        Arc::new(AtomicUsize::new(0)),
    );
    let desk = craft_ai_client::ToolDesk::new(c, shared.clone(), None);
    let refused = desk.choose(
        &role_for(cap),
        &"one".parse().unwrap(),
        &ModelId::new("not_a_real_model").unwrap(),
    );
    assert_eq!(refused.unwrap_err().kind, ErrorKind::Unsupported);
    assert_eq!(std::fs::read(shared.path()).unwrap(), before);
}

/// Merges `extra` into the first saved model's capability, for limits `saved_at` does not set.
fn declare(shared: &SharedStore, extra: serde_json::Value) {
    let mut file: serde_json::Value =
        serde_json::from_slice(&std::fs::read(shared.path()).unwrap()).unwrap();
    let spec = &mut file["connections"][0]["models"][0]["capabilities"][0];
    for (key, value) in extra.as_object().unwrap() {
        spec[key] = value.clone();
    }
    std::fs::write(shared.path(), file.to_string()).unwrap();
}

/// Options, more outputs and streaming are refused at plan when the listed model refuses them, even
/// though the saved spec allows them: the adapter checks its own list when it submits.
#[test]
fn options_outputs_and_streaming_the_listed_model_refuses_are_refused_at_plan() {
    let dir = tempfile::tempdir().unwrap();
    let cap = "image.generate";
    let shared = saved_at(
        &dir,
        "artcraft-omni",
        LOOPBACK,
        "nano_banana",
        cap,
        json!({"prompt": 1}),
    );
    declare(
        &shared,
        json!({"max_outputs": 2, "features": {"streaming": true}}),
    );
    let reads = Arc::new(AtomicUsize::new(0));
    let c = counted_client(shared, ProviderFactory::standard(), reads.clone());
    assert!(c.plan(&role_for(cap), request_for(cap)).is_ok());
    for (what, request) in [
        ("options", request_for(cap).with_param("seed", json!(7))),
        ("two outputs", request_for(cap).with_outputs(2)),
        ("streaming", request_for(cap).streaming()),
    ] {
        let refused = c.plan(&role_for(cap), request).unwrap_err();
        assert_eq!(refused.kind, ErrorKind::Unsupported, "{what}");
    }
    assert_eq!(reads.load(Ordering::SeqCst), 0, "no key was read");
}

/// Vertex express takes one output and no options, so an option is refused at plan.
#[test]
fn vertex_refuses_options_at_plan() {
    let dir = tempfile::tempdir().unwrap();
    let cap = "image.generate";
    let shared = saved_at(
        &dir,
        "vertex-express",
        LOOPBACK,
        "gemini-2.5-flash-image",
        cap,
        json!({"prompt": 1}),
    );
    let c = client(shared, ProviderFactory::standard());
    assert!(c.plan(&role_for(cap), request_for(cap)).is_ok());
    let with_option = request_for(cap).with_param("seed", json!(7));
    let refused = c.plan(&role_for(cap), with_option).unwrap_err();
    assert_eq!(refused.kind, ErrorKind::Unsupported);
}

/// A provider this app has no adapter for is not offered or bound for this app, but the suite
/// default can still be set: the launcher may set up a model that only some apps can run.
#[test]
fn a_provider_with_no_adapter_is_refused_for_this_app_but_the_suite_default_stays_open() {
    let dir = tempfile::tempdir().unwrap();
    let cap = "image.generate";
    let shared = saved_at(
        &dir,
        "not-installed",
        LOOPBACK,
        "any",
        cap,
        json!({"prompt": 1}),
    );
    let c = client(shared.clone(), ProviderFactory::standard());
    assert!(c.choices(&role()).unwrap().is_empty());
    assert!(c.picks(&role()).unwrap().is_empty());
    let before = std::fs::read(shared.path()).unwrap();
    let refused = c
        .bind(
            &role(),
            &"one".parse().unwrap(),
            &ModelId::new("other").unwrap(),
        )
        .unwrap_err();
    assert_eq!(refused.kind, ErrorKind::Unsupported);
    assert_eq!(std::fs::read(shared.path()).unwrap(), before);

    let desk = craft_ai_client::ToolDesk::new(c, shared.clone(), None);
    desk.choose(
        &role(),
        &"one".parse().unwrap(),
        &ModelId::new("other").unwrap(),
    )
    .unwrap();
    let after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(shared.path()).unwrap()).unwrap();
    assert_eq!(after["defaults"][0]["model"], "other");
}
