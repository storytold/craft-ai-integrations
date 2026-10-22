use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use craft_ai_core::segment::{self, Area, Click};
use craft_ai_core::{ErrorKind, Payload, Request};
use craft_ai_engine::RunContext;
use craft_ai_engine::testing::check_runner;
use craft_ai_engine_rten::picture::decode_picture;
use craft_ai_engine_rten::resample::{MASK_SIDE, SIDE};
use craft_ai_engine_rten::{Backend, Prediction};
use png::{BitDepth, ColorType};
use serde_json::json;

use super::*;

/// The `image.segment` capability.
fn capability() -> CapabilityId {
    CapabilityId::parse("image.segment").unwrap()
}

/// A flat grey PNG, `w` by `h`.
fn picture(w: u32, h: u32, grey: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, w, h);
    e.set_color(ColorType::Rgb);
    e.set_depth(BitDepth::Eight);
    let mut writer = e.write_header().unwrap();
    writer
        .write_image_data(&vec![grey; (w * h * 3) as usize])
        .unwrap();
    writer.finish().unwrap();
    out
}

/// An `image.segment` request for the PNG `png` with one positive click at (`x`, `y`).
fn request(png: Vec<u8>, x: f64, y: f64) -> Request {
    Request::new(capability())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", png).unwrap(),
        )
        .with_param("points", json!([{"x": x, "y": y, "positive": true}]))
}

/// What the stand-in was asked: the clicks and the box of one request.
type Asked = (Vec<Click>, Option<Area>);

/// A stand-in for the graphics card: counts pictures, records what it is asked and answers a
/// fixed mask, or fails on purpose.
#[derive(Clone)]
struct Fake {
    encodes: Arc<AtomicUsize>,
    asked: Arc<Mutex<Vec<Asked>>>,
    logits: Vec<f32>,
    fail: bool,
}

impl Fake {
    /// Answers `logits` to every request.
    fn answering(logits: Vec<f32>) -> Self {
        Self {
            encodes: Arc::default(),
            asked: Arc::default(),
            logits,
            fail: false,
        }
    }

    /// Selects the left half of the mask.
    fn left_half() -> Self {
        Self::answering(
            (0..MASK_SIDE * MASK_SIDE)
                .map(|i| {
                    if i % MASK_SIDE < MASK_SIDE / 2 {
                        9.0
                    } else {
                        -9.0
                    }
                })
                .collect(),
        )
    }
}

impl Backend for Fake {
    type Features = ();

    /// Counts the picture, refusing one that is not the model's 1024 square.
    fn encode(&self, picture: Vec<f32>) -> Result<(), AiError> {
        assert_eq!(picture.len(), 3 * SIDE * SIDE);
        self.encodes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Records the question and answers the fixed mask, or the planned failure.
    fn segment(&self, _: &(), clicks: &[Click], area: Option<Area>) -> Result<Prediction, AiError> {
        self.asked.lock().unwrap().push((clicks.to_vec(), area));
        if self.fail {
            return Err(AiError::new(ErrorKind::Provider, "the card went away"));
        }
        Ok(Prediction {
            logits: self.logits.clone(),
            score: 0.8,
        })
    }
}

/// Runs one request on a loaded stand-in with no cancellation.
fn run(model: &mut Loaded<Fake>, req: &Request) -> Result<Vec<craft_ai_core::Output>, AiError> {
    model.run(req, &RunContext::detached())
}

/// The first output decoded as a PNG picture.
fn mask_of(out: &[craft_ai_core::Output]) -> craft_ai_engine_rten::picture::Rgb {
    let Payload::Inline { data, .. } = &out[0].payload else {
        panic!("an inline mask is expected");
    };
    decode_picture(data).unwrap()
}

/// The runner is engine `burn`, model `sam21`, needs a source picture, accepts a prompt, and
/// declares the card makers it reaches.
#[test]
fn the_runner_is_the_burn_sam21_the_catalogue_names() {
    assert_eq!((ENGINE, MODEL), ("burn", "sam21"));
    let runner = Sam21Runner;
    assert_eq!(runner.engine(), "burn");
    let info = runner.model();
    assert_eq!(info.id.as_str(), "sam21");
    let spec = &info.capabilities[0];
    assert_eq!(spec.capability, capability());
    assert_eq!(spec.requires, vec![InputRole::Source]);
    assert!(spec.accepts.contains_key(&InputRole::Prompt));
    assert!(runner.accelerators().contains(&GpuVendor::Nvidia));
    assert_eq!(runners().len(), 1);
}

/// The downloaded ONNX file is not what this engine reads, and it is not the supported export
/// either: the engine refuses it in plain words (no path, no crash) instead of converting it.
#[cfg(feature = "prepare")]
#[test]
fn an_onnx_file_that_is_not_the_supported_export_is_refused_plainly() {
    // The saved copy goes beside the model folder, so the folder is one level inside the test's own
    // temp folder and nothing lands in the shared system temp folder.
    let root = tempfile::tempdir().unwrap();
    let models = root.path().join("models");
    std::fs::create_dir(&models).unwrap();
    std::fs::write(models.join("encoder.onnx"), b"x").unwrap();
    std::fs::write(models.join("decoder.onnx"), b"x").unwrap();
    let error = Sam21Runner
        .load(&models.join("encoder.onnx"))
        .err()
        .unwrap();
    assert_eq!(error.kind, ErrorKind::Unsupported);
    assert!(
        !error.message.contains(root.path().to_str().unwrap()),
        "{}",
        error.message
    );
    assert!(!error.message.is_empty());
    // The refusal is about the files: not the stale "prepared files" answer, and not about where the
    // saved copy would go.
    assert!(!error.message.contains("encoder.bpk"), "{}", error.message);
    assert!(
        !error.message.contains("separate folder"),
        "{}",
        error.message
    );
    // Nothing was written beside the folder.
    assert!(
        !root
            .path()
            .join("burn-prepared")
            .join("encoder.bpk")
            .exists()
    );
}

/// A damaged saved copy is explained with where it lives and what to do, since trying again cannot
/// help; any other failure passes through unchanged.
#[cfg(feature = "prepare")]
#[test]
fn a_damaged_saved_copy_is_explained_with_where_it_lives() {
    let damaged = prepare::explain(craft_ai_core::AiError::unsupported(
        craft_ai_burn_setup::DAMAGED_COPY,
    ));
    assert_eq!(damaged.kind, ErrorKind::Unsupported);
    assert!(
        damaged.message.contains("burn-prepared"),
        "{}",
        damaged.message
    );
    assert!(damaged.message.contains("Delete"), "{}", damaged.message);
    assert!(
        !damaged.message.contains("try again."),
        "{}",
        damaged.message
    );
    let other = craft_ai_core::AiError::unsupported("Another app is preparing this model.");
    assert_eq!(prepare::explain(other.clone()).message, other.message);
}

/// With the converter left out of the build, a downloaded file is answered with where to go.
#[cfg(not(feature = "prepare"))]
#[test]
fn without_the_converter_a_downloaded_file_says_to_open_set_up() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("encoder.onnx"), b"x").unwrap();
    let error = Sam21Runner
        .load(&dir.path().join("encoder.onnx"))
        .err()
        .unwrap();
    assert!(error.message.contains("Set up AI"), "{}", error.message);
}

/// A downloaded encoder with no decoder beside it says which files are needed.
#[cfg(feature = "prepare")]
#[test]
fn a_downloaded_encoder_with_no_decoder_says_what_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("encoder.onnx"), b"x").unwrap();
    let error = Sam21Runner
        .load(&dir.path().join("encoder.onnx"))
        .err()
        .unwrap();
    assert_eq!(error.kind, ErrorKind::Unsupported);
    assert!(error.message.contains("decoder.onnx"), "{}", error.message);
    assert!(error.message.contains("Set up AI"), "{}", error.message);
}

/// An encoder with no decoder beside it, a different file name and a missing file each say which
/// files are needed.
#[test]
fn weights_that_are_not_both_files_in_one_folder_say_what_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let encoder = dir.path().join(ENCODER_FILE);
    std::fs::write(&encoder, b"NRUB").unwrap();
    let other = dir.path().join("other.bpk");
    std::fs::write(&other, b"NRUB").unwrap();
    std::fs::write(dir.path().join(DECODER_FILE), b"NRUB").unwrap();
    for weights in [other, dir.path().join("missing").join(ENCODER_FILE)] {
        let error = Sam21Runner.load(&weights).err().unwrap();
        assert_eq!(error.kind, ErrorKind::Unsupported);
        assert!(error.message.contains("encoder.bpk"), "{}", error.message);
    }
    std::fs::remove_file(dir.path().join(DECODER_FILE)).unwrap();
    let error = Sam21Runner.load(&encoder).err().unwrap();
    assert!(error.message.contains("decoder.bpk"), "{}", error.message);
}

/// Both files present and marked as saved weights but damaged: an error, not a crash.
#[test]
fn damaged_prepared_files_are_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    for name in [ENCODER_FILE, DECODER_FILE] {
        std::fs::write(dir.path().join(name), b"NRUB definitely not the model").unwrap();
    }
    let error = Sam21Runner
        .load(&dir.path().join(ENCODER_FILE))
        .err()
        .unwrap();
    assert_eq!(error.kind, ErrorKind::Unsupported);
    assert!(error.message.contains("Set up AI"), "{}", error.message);
}

/// One click and one box reach the model as fractions of the picture, in order, whichever
/// engine's request handling is behind them.
#[test]
fn click_and_box_reach_the_model_as_fractions_of_the_picture() {
    let fake = Fake::left_half();
    let mut model = Loaded::new(fake.clone());
    let boxed = Area {
        x0: 0.1,
        y0: 0.2,
        x1: 0.6,
        y1: 0.9,
    };
    run(&mut model, &request(picture(8, 8, 1), 0.25, 0.75)).unwrap();
    let with_box = segment::with_area(request(picture(8, 8, 1), 0.5, 0.5), boxed);
    run(&mut model, &with_box).unwrap();
    let asked = fake.asked.lock().unwrap();
    let click = Click {
        x: 0.25,
        y: 0.75,
        positive: true,
    };
    assert_eq!(asked[0], (vec![click], None));
    assert_eq!(asked[1].1, Some(boxed));
    assert_eq!(
        fake.encodes.load(Ordering::SeqCst),
        1,
        "one picture, one look"
    );
}

/// The mask comes back at the picture's own size, white where selected.
#[test]
fn the_mask_is_at_the_size_of_the_picture() {
    let mut model = Loaded::new(Fake::left_half());
    let out = run(&mut model, &request(picture(40, 20, 100), 0.2, 0.5)).unwrap();
    let mask = mask_of(&out);
    assert_eq!((mask.width, mask.height), (40, 20));
    assert!(mask.bytes[(10 * 40 + 2) * 3] > 250, "left selected");
    assert!(mask.bytes[(10 * 40 + 37) * 3] < 5, "right not");
    assert!(segment::read_found(&out).unwrap().matched);
}

/// When the model selects nothing the answer says so and the mask is empty at the picture's size.
#[test]
fn an_empty_mask_is_no_match() {
    let mut model = Loaded::new(Fake::answering(vec![-8.0; MASK_SIDE * MASK_SIDE]));
    let out = run(&mut model, &request(picture(10, 6, 7), 0.5, 0.5)).unwrap();
    let found = segment::read_found(&out).unwrap();
    assert!(!found.matched);
    assert_eq!(found.score, None);
    let mask = mask_of(&out);
    assert_eq!((mask.width, mask.height), (10, 6));
    assert!(mask.bytes.iter().all(|b| *b == 0));
}

/// A mask shorter than the model promises is no match, and whatever the model returns the mask the
/// person gets is the size of their picture. (The real backend refuses a wrong length before it
/// gets here; see `an_answer_of_the_wrong_size_is_an_error_not_a_panic`.)
#[test]
fn a_short_mask_is_no_match_at_the_size_of_the_picture() {
    for len in [0, 1, MASK_SIDE * MASK_SIDE - 1] {
        let mut model = Loaded::new(Fake::answering(vec![5.0; len]));
        let out = run(&mut model, &request(picture(8, 4, 1), 0.5, 0.5)).unwrap();
        assert!(!segment::read_found(&out).unwrap().matched);
        let mask = mask_of(&out);
        assert_eq!((mask.width, mask.height), (8, 4));
        assert!(mask.bytes.iter().all(|b| *b == 0));
    }
}

/// A failure inside the model is the job's error, not a crash, and the next request is served.
#[test]
fn a_failure_in_the_model_is_an_error_and_the_next_request_still_runs() {
    let mut fake = Fake::left_half();
    fake.fail = true;
    let mut model = Loaded::new(fake);
    let error = run(&mut model, &request(picture(8, 8, 1), 0.5, 0.5)).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Provider);
    let again = run(&mut model, &request(picture(8, 8, 1), 0.5, 0.5)).unwrap_err();
    assert_eq!(again.kind, ErrorKind::Provider);
}

/// A runner built on the stand-in passes the shared conformance run, which includes Cancel while a
/// job runs, a refused request and a second job after a Cancel.
#[test]
fn it_passes_the_shared_runner_checks_including_cancel() {
    struct FakeRunner(Fake);
    impl ModelRunner for FakeRunner {
        /// The engine name.
        fn engine(&self) -> &str {
            ENGINE
        }
        /// The real runner's model description.
        fn model(&self) -> ModelInfo {
            Sam21Runner.model()
        }
        /// Loads the stand-in whatever file is named.
        fn load(&self, _: &Path) -> Result<Box<dyn LoadedModel>, AiError> {
            Ok(Box::new(Loaded::new(self.0.clone())))
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let weights = dir.path().join(ENCODER_FILE);
    std::fs::write(&weights, b"x").unwrap();
    let good = request(picture(24, 12, 80), 0.3, 0.5);
    let bad = Request::new(capability()).with_param("points", json!([]));
    check_runner(
        Arc::new(FakeRunner(Fake::left_half())),
        &weights,
        &good,
        &bad,
        ErrorKind::InvalidRequest,
    )
    .unwrap();
}
