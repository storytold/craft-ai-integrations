//! How fast hover can be through the ordinary job path: one picture, then a mask for each mouse
//! position, each a normal `plan`/`start`/`wait` round trip with the real SAM 2.1 files on rten.
//! It measures, and fails only if a hover is slower than a person would accept.
//!
//! Skips unless `CRAFT_TEST_SAM21_ONNX_DIR` and `CRAFT_TEST_SAM21_IMAGE` are set (see `rten_live`).
//! Run: `cargo test -p craft-ai-client --features rten --release --test rten_hover_live -- --nocapture`.
#![cfg(feature = "rten")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use craft_ai_client::{AiClient, ToolDesk};
use craft_ai_core::{AppId, ConsentLedger, InputRole, JobState, Payload, Request, StoreFile};
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;
use serde_json::json;

/// The encoder weights and the sample picture, or `None` when the environment does not name them.
fn files() -> Option<(PathBuf, Vec<u8>)> {
    let dir = PathBuf::from(std::env::var_os("CRAFT_TEST_SAM21_ONNX_DIR")?);
    let image = std::fs::read(std::env::var_os("CRAFT_TEST_SAM21_IMAGE")?).ok()?;
    Some((dir.join("encoder.onnx"), image))
}

/// The time below which a hover feels live; past it the selection visibly trails the mouse.
const HOVER_BUDGET: Duration = Duration::from_millis(150);

/// After the first look at the picture, every hover position through the ordinary job path
/// returns its mask within [`HOVER_BUDGET`] at the median, and the timings are printed.
#[test]
fn hover_through_the_job_path_is_fast_enough_after_the_first_look() {
    let Some((weights, image)) = files() else {
        eprintln!("skipped: set CRAFT_TEST_SAM21_ONNX_DIR and CRAFT_TEST_SAM21_IMAGE");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    shared.replace(&StoreFile::default()).unwrap();
    let client = |app: &str| {
        AiClient::new(
            AppId::new(app).unwrap(),
            shared.clone(),
            Arc::new(MemoryStore::new()),
        )
    };
    let feature = craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    let role = feature.role().unwrap();
    ToolDesk::new(client("launcher"), shared.clone(), None)
        .enable_native_with(&feature, "rten", &weights)
        .unwrap();
    let app = client("photocraft");
    let source = Payload::inline("image/png", image).unwrap();
    let ask = |x: f64, y: f64| {
        let request = Request::new(feature.capability.clone())
            .with_input(InputRole::Source, source.clone())
            .with_param("points", json!([{"x": x, "y": y, "positive": true}]));
        let started = Instant::now();
        let plan = app.plan(&role, request).unwrap();
        let done = app
            .start(&plan, &ConsentLedger::default())
            .unwrap()
            .wait(Duration::from_millis(1), Duration::from_secs(120))
            .unwrap();
        assert_eq!(done.state, JobState::Succeeded);
        started.elapsed()
    };
    let first = ask(0.29, 0.64);
    let mut hovers: Vec<Duration> = (0..30)
        .map(|i| {
            let t = f64::from(i) / 30.0;
            ask(0.2 + 0.6 * t, 0.3 + 0.5 * t)
        })
        .collect();
    hovers.sort();
    let median = hovers[hovers.len() / 2];
    let slowest = hovers[hovers.len() - 1];
    println!(
        "first look {first:?}; hover median {median:?}, fastest {:?}, slowest {slowest:?}",
        hovers[0]
    );
    assert!(median <= HOVER_BUDGET, "hover median {median:?}");
}
