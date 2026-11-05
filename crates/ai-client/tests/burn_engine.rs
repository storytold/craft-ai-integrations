//! The Burn engine through the client: the catalogue's route, the shared store, the cargo feature
//! and the engine. An app writes none of it, and two apps read the same choice.
//!
//! The first test needs no model files. The second (ignored) needs the real ones:
//! `CRAFT_TEST_SAM21_BURN_DIR` (the folder `craft-ai-burn-setup` saved, holding `encoder.bpk` and
//! `decoder.bpk`) and `CRAFT_TEST_SAM21_IMAGE`. Run:
//! `cargo test -p craft-ai-client --features burn --release --test burn_engine -- --ignored`.
#![cfg(feature = "burn")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use craft_ai_client::{AiClient, ToolDesk};
use craft_ai_core::{
    AppId, ConsentLedger, Event, InputRole, JobState, Payload, Request, StoreFile, segment,
};
use craft_ai_features::Feature;
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;
use serde_json::json;

/// The built-in `select-subject` feature.
fn feature() -> Feature {
    craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap()
}

/// A shared store in a temporary folder, with the launcher's desk over it.
struct World {
    _dir: tempfile::TempDir,
    shared: SharedStore,
}

impl World {
    /// An empty shared store.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let shared = SharedStore::new(dir.path().join("connections.json"));
        shared.replace(&StoreFile::default()).unwrap();
        Self { _dir: dir, shared }
    }

    /// An `AiClient` for the app `app` over the shared store.
    fn client(&self, app: &str) -> AiClient {
        AiClient::new(
            AppId::new(app).unwrap(),
            self.shared.clone(),
            Arc::new(MemoryStore::new()),
        )
    }

    /// The launcher's part: choose Burn for the tool with `weights` as its file.
    fn choose_burn(&self, weights: &Path) {
        ToolDesk::new(self.client("launcher"), self.shared.clone(), None)
            .enable_native_with(&feature(), "burn", weights)
            .unwrap();
    }
}

/// A one-click `image.segment` request for `image`.
fn request(image: Vec<u8>) -> Request {
    Request::new(feature().capability.clone())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", image).unwrap(),
        )
        .with_param("points", json!([{"x": 0.29, "y": 0.64, "positive": true}]))
}

/// The cargo feature is the whole of adopting the engine: a connection on the Burn engine is built,
/// and one that holds a download with no decoder is told, in plain words, what is missing instead
/// of being reported as an engine the app does not have.
#[test]
fn the_engine_is_known_and_an_incomplete_download_says_what_is_missing() {
    let w = World::new();
    let dir = tempfile::tempdir().unwrap();
    let weights = dir.path().join("encoder.onnx");
    std::fs::write(&weights, b"not the model").unwrap();
    w.choose_burn(&weights);

    let role = feature().role().unwrap();
    let app = w.client("lightcraft");
    // The refusal may come when the plan is made, when the job starts or as the job's own end.
    let error = match app.plan(&role, request(vec![1, 2, 3])) {
        Err(e) => e,
        Ok(plan) => match app.start(&plan, &ConsentLedger::default()) {
            Err(e) => e,
            Ok(job) => {
                match job
                    .wait(Duration::from_millis(20), Duration::from_secs(30))
                    .unwrap()
                    .state
                {
                    JobState::Failed { error } => error,
                    other => panic!("expected a refusal, got {other:?}"),
                }
            }
        },
    };
    // Only the encoder was written, so the answer names the missing decoder and where to go.
    assert!(error.message.contains("decoder.onnx"), "{}", error.message);
    assert!(error.message.contains("Set up AI"), "{}", error.message);
}

/// Two apps ask only for the capability and each gets the real model's mask of the suit at the
/// picture's size, with the shared result saying it matched.
#[test]
#[ignore = "needs the prepared SAM 2.1 files; see the module docs"]
fn two_apps_get_a_mask_from_the_real_model_by_asking_for_select_subject() {
    let (Some(burn), Some(image)) = (
        std::env::var_os("CRAFT_TEST_SAM21_BURN_DIR"),
        std::env::var_os("CRAFT_TEST_SAM21_IMAGE"),
    ) else {
        eprintln!("skipped: set CRAFT_TEST_SAM21_BURN_DIR and CRAFT_TEST_SAM21_IMAGE");
        return;
    };
    let image = std::fs::read(image).unwrap();
    let w = World::new();
    w.choose_burn(&PathBuf::from(burn).join("encoder.bpk"));
    let role = feature().role().unwrap();
    for app in ["lightcraft", "photocraft"] {
        let app = w.client(app);
        let plan = app.plan(&role, request(image.clone())).unwrap();
        let done = app
            .start(&plan, &ConsentLedger::default())
            .unwrap()
            .wait(Duration::from_millis(50), Duration::from_secs(180))
            .unwrap();
        assert_eq!(done.state, JobState::Succeeded);
        let outputs: Vec<_> = done
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Output(o) => Some(o.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(outputs.len(), 2, "the mask, then the shared result");
        assert!(segment::read_found(&outputs).unwrap().matched);
        let Some(Payload::Inline { data, .. }) = segment::mask(&outputs) else {
            panic!("expected the mask itself");
        };
        let mut reader = png::Decoder::new(std::io::Cursor::new(data))
            .read_info()
            .unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut pixels).unwrap();
        assert_eq!((frame.width, frame.height), (512, 512));
        let selected = pixels.iter().filter(|v| **v > 127).count() as f64 / (512.0 * 512.0);
        assert!((0.15..0.35).contains(&selected), "selected {selected}");
    }
}
