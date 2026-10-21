//! The whole chain with the real model: the catalogue's route, the shared store, the cargo feature,
//! the rten engine and the publisher's ONNX files. An app writes none of it, and two apps read
//! the same choice.
//!
//! Skips unless `CRAFT_TEST_SAM21_ONNX_DIR` names a folder holding `encoder.onnx` and
//! `decoder.onnx`, and `CRAFT_TEST_SAM21_IMAGE` names a picture. Run:
//! `cargo test -p craft-ai-client --features rten --release --test rten_live`.
#![cfg(feature = "rten")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use craft_ai_client::{AiClient, ToolDesk};
use craft_ai_core::{
    AppId, ConsentLedger, Event, InputRole, JobState, Payload, Request, StoreFile, segment,
};
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;
use serde_json::json;

/// The encoder path and the sample picture named in the environment, or `None` when they are not
/// set.
fn files() -> Option<(PathBuf, Vec<u8>)> {
    let dir = PathBuf::from(std::env::var_os("CRAFT_TEST_SAM21_ONNX_DIR")?);
    let image = std::fs::read(std::env::var_os("CRAFT_TEST_SAM21_IMAGE")?).ok()?;
    Some((dir.join("encoder.onnx"), image))
}

/// Two apps ask only for the capability and each gets the real model's mask of the suit, at the
/// picture's size, with the shared result saying it matched.
#[test]
fn two_apps_get_a_mask_from_the_real_model_by_asking_for_select_subject() {
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

    // The launcher's part, once the files are in place: rten's recipe is SAM 2.1.
    ToolDesk::new(client("launcher"), shared.clone(), None)
        .enable_native_with(&feature, "rten", &weights)
        .unwrap();

    // Each app's part: ask for the capability. Neither is told which engine, model or file.
    for app in ["lightcraft", "photocraft"] {
        let app = client(app);
        let request = Request::new(feature.capability.clone())
            .with_input(
                InputRole::Source,
                Payload::inline("image/png", image.clone()).unwrap(),
            )
            .with_param("points", json!([{"x": 0.29, "y": 0.64, "positive": true}]));
        let plan = app.plan(&role, request).unwrap();
        let done = app
            .start(&plan, &ConsentLedger::default())
            .unwrap()
            .wait(Duration::from_millis(50), Duration::from_secs(120))
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
        // The mask, then the shared result saying it matched.
        assert_eq!(outputs.len(), 2);
        assert!(segment::read_found(&outputs).unwrap().matched);
        let Some(Payload::Inline { media_type, data }) = segment::mask(&outputs) else {
            panic!("expected the mask itself");
        };
        assert_eq!(media_type.as_str(), "image/png");
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
