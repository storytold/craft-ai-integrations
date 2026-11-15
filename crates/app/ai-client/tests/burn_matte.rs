//! Remove background on the Burn engine through the client: the catalogue's built-in route, the
//! shared store, the cargo feature and the InSPyReNet runner. An app asks for `image.matte` only.
//!
//! The first test needs no model file. The second (ignored) needs the real ones:
//! `CRAFT_TEST_INSPYRENET_WEIGHTS` (`inspyrenet.safetensors`) and `CRAFT_TEST_INSPYRENET_IMAGE` (a
//! PNG). Run:
//! `cargo test -p craft-ai-client --features burn --release --test burn_matte -- --ignored`.
#![cfg(feature = "burn")]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use craft_ai_client::{AiClient, ToolDesk};
use craft_ai_core::{
    AiError, AppId, ConsentLedger, Event, InputRole, JobState, Payload, Request, StoreFile,
};
use craft_ai_features::Feature;
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;

fn remove_background() -> Feature {
    craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == "remove-background")
        .unwrap()
}

struct World {
    _dir: tempfile::TempDir,
    shared: SharedStore,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let shared = SharedStore::new(dir.path().join("connections.json"));
        shared.replace(&StoreFile::default()).unwrap();
        Self { _dir: dir, shared }
    }

    fn client(&self, app: &str) -> AiClient {
        AiClient::new(
            AppId::new(app).unwrap(),
            self.shared.clone(),
            Arc::new(MemoryStore::new()),
        )
    }

    /// The launcher's part: set up the tool's built-in route with `weights` as its file.
    fn set_up(&self, weights: &Path) {
        ToolDesk::new(self.client("launcher"), self.shared.clone(), None)
            .enable_native(&remove_background(), weights)
            .unwrap();
    }
}

fn request(picture: Vec<u8>) -> Request {
    Request::new(remove_background().capability.clone()).with_input(
        InputRole::Source,
        Payload::inline("image/png", picture).unwrap(),
    )
}

/// Runs `request` for `app` to its end: the outputs, or the refusal from wherever it came.
fn run(app: &AiClient, request: Request) -> Result<Vec<Payload>, AiError> {
    let role = remove_background().role().unwrap();
    let plan = app.plan(&role, request)?;
    let done = app
        .start(&plan, &ConsentLedger::default())?
        .wait(Duration::from_millis(50), Duration::from_secs(600))?;
    match done.state {
        JobState::Succeeded => Ok(done
            .events
            .into_iter()
            .filter_map(|e| match e {
                Event::Output(o) => Some(o.payload),
                _ => None,
            })
            .collect()),
        JobState::Failed { error } => Err(error),
        other => panic!("unexpected end {other:?}"),
    }
}

/// A file that is not the checkpoint is refused in plain words that point to Set up AI.
#[test]
fn a_file_that_is_not_the_checkpoint_is_refused_in_words() {
    let w = World::new();
    let dir = tempfile::tempdir().unwrap();
    let weights = dir.path().join("inspyrenet.safetensors");
    std::fs::write(&weights, b"not the model").unwrap();
    w.set_up(&weights);
    let error = run(&w.client("photocraft"), request(vec![1, 2, 3])).unwrap_err();
    assert!(error.message.contains("Set up AI"), "{}", error.message);
}

/// Two apps ask only for Remove background and each gets one grayscale mask at the picture's size
/// with both subject and background in it.
#[test]
#[ignore = "needs CRAFT_TEST_INSPYRENET_WEIGHTS and CRAFT_TEST_INSPYRENET_IMAGE; see the module docs"]
fn two_apps_get_a_matte_from_the_real_model() {
    let (Some(weights), Some(image)) = (
        std::env::var_os("CRAFT_TEST_INSPYRENET_WEIGHTS"),
        std::env::var_os("CRAFT_TEST_INSPYRENET_IMAGE"),
    ) else {
        eprintln!("skipped: set CRAFT_TEST_INSPYRENET_WEIGHTS and CRAFT_TEST_INSPYRENET_IMAGE");
        return;
    };
    let picture = std::fs::read(image).unwrap();
    let size = png::Decoder::new(std::io::Cursor::new(&picture))
        .read_info()
        .unwrap()
        .info()
        .size();
    let w = World::new();
    w.set_up(Path::new(&weights));
    for app in ["photocraft", "lightcraft"] {
        let started = std::time::Instant::now();
        let outputs = run(&w.client(app), request(picture.clone())).unwrap();
        eprintln!("{app}: {:.1?}", started.elapsed());
        assert_eq!(outputs.len(), 1);
        let Payload::Inline { data, .. } = &outputs[0] else {
            panic!("expected the mask itself");
        };
        let mut reader = png::Decoder::new(std::io::Cursor::new(data))
            .read_info()
            .unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let frame = reader.next_frame(&mut pixels).unwrap();
        assert_eq!((frame.width, frame.height), size);
        assert_eq!(frame.color_type, png::ColorType::Grayscale);
        let subject = pixels.iter().filter(|v| **v > 127).count() as f64 / pixels.len() as f64;
        assert!((0.02..0.98).contains(&subject), "subject {subject}");
    }
}
