use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use craft_ai_core::segment::{self, Area, MAX_POINTS};
use craft_ai_core::{CapabilityId, ErrorKind, InputRole, Payload, Request};
use craft_ai_engine::RunContext;
use craft_ai_engine::testing::check_runner;
use png::{BitDepth, ColorType};
use serde_json::json;

use super::*;
use crate::picture::decode_picture;
use crate::resample::{MASK_SIDE, SIDE};
use crate::sam21::Prediction;

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

/// One click as the JSON a request carries.
fn click(x: f64, y: f64, positive: bool) -> serde_json::Value {
    json!({"x": x, "y": y, "positive": positive})
}

/// An `image.segment` request for the PNG `png` with `points` as its clicks.
fn request(png: Vec<u8>, points: serde_json::Value) -> Request {
    Request::new(capability())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", png).unwrap(),
        )
        .with_param("points", points)
}

/// The clicks and box one request put to the model.
type Asked = (Vec<Click>, Option<Area>);

/// A stand-in for the model that records what it was asked and answers a fixed mask.
#[derive(Clone)]
struct Fake {
    encodes: Arc<AtomicUsize>,
    asked: Arc<Mutex<Vec<Asked>>>,
    logits: Vec<f32>,
    score: f32,
}

impl Fake {
    /// Left half of the mask selected, right half not.
    fn left_half() -> Self {
        let logits = (0..MASK_SIDE * MASK_SIDE)
            .map(|i| {
                if i % MASK_SIDE < MASK_SIDE / 2 {
                    12.0
                } else {
                    -12.0
                }
            })
            .collect();
        Self::answering(logits)
    }

    /// A stand-in that answers `logits` to every request.
    fn answering(logits: Vec<f32>) -> Self {
        Self {
            encodes: Arc::default(),
            asked: Arc::default(),
            logits,
            score: 0.9,
        }
    }
}

impl Backend for Fake {
    type Features = ();

    /// Counts the picture, after checking it arrives as the model's 1024 square.
    fn encode(&self, picture: Vec<f32>) -> Result<(), AiError> {
        assert_eq!(
            picture.len(),
            3 * SIDE * SIDE,
            "the model is given its 1024 square"
        );
        self.encodes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Records the clicks and box it was asked about and answers the fixed mask.
    fn segment(&self, _: &(), clicks: &[Click], area: Option<Area>) -> Result<Prediction, AiError> {
        self.asked.lock().unwrap().push((clicks.to_vec(), area));
        Ok(Prediction {
            logits: self.logits.clone(),
            score: self.score,
        })
    }
}

/// Runs one request on a loaded stand-in with no cancellation.
fn run(model: &mut Loaded<Fake>, req: &Request) -> Result<Vec<craft_ai_core::Output>, AiError> {
    model.run(req, &RunContext::detached())
}

/// The answer's first output decoded as a PNG picture.
fn mask_of(out: &[craft_ai_core::Output]) -> crate::picture::Rgb {
    let Payload::Inline { data, media_type } = &out[0].payload else {
        panic!("an inline mask is expected");
    };
    assert_eq!(media_type.as_str(), "image/png");
    decode_picture(data).unwrap()
}

/// The runner is engine `rten`, model `sam21`, needs a source picture, accepts a prompt and uses no
/// accelerator.
#[test]
fn the_runner_is_the_rten_sam21_the_catalogue_names_and_needs_only_a_picture() {
    let runner = Sam21Runner;
    assert_eq!(runner.engine(), "rten");
    let info = runner.model();
    assert_eq!(info.id.as_str(), "sam21");
    let spec = info.spec_for(&capability()).unwrap();
    assert!(spec.requires.contains(&InputRole::Source));
    assert!(!spec.requires.contains(&InputRole::Prompt));
    // The tool offers a prompt input, so the model must accept one to be chosen for it.
    assert!(
        spec.accepts
            .iter()
            .any(|(role, _)| *role == InputRole::Prompt)
    );
    assert!(
        runner.accelerators().is_empty(),
        "this engine runs on the processor"
    );
}

/// Without a box, a request needs clicks and one of them on the object; good clicks read back
/// as given.
#[test]
fn clicks_are_needed_and_one_must_be_on_the_object() {
    let png = picture(4, 4, 9);
    let none = Request::new(capability()).with_input(
        InputRole::Source,
        Payload::inline("image/png", png.clone()).unwrap(),
    );
    assert_eq!(
        read_prompt(&none).unwrap_err().kind,
        ErrorKind::InvalidRequest
    );
    let only_off = request(png.clone(), json!([click(0.5, 0.5, false)]));
    assert!(read_prompt(&only_off).is_err());
    let ok = request(
        png,
        json!([click(0.25, 0.75, true), click(0.5, 0.5, false)]),
    );
    let clicks = read_prompt(&ok).unwrap().0;
    assert_eq!(clicks.len(), 2);
    assert_eq!(
        (clicks[0].x, clicks[0].y, clicks[0].positive),
        (0.25, 0.75, true)
    );
}

/// A description is refused in plain words that point at Set up AI; a blank prompt is no
/// description.
#[test]
fn a_description_is_refused_in_plain_words_not_answered_with_a_guess() {
    let png = picture(4, 4, 9);
    let words = request(png.clone(), json!([click(0.5, 0.5, true)]))
        .with_input(InputRole::Prompt, Payload::text("the dog"));
    let e = read_prompt(&words).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert!(
        e.message.contains("clicking") && e.message.contains("Set up AI"),
        "{}",
        e.message
    );
    // An empty prompt is no prompt.
    let blank = request(png, json!([click(0.5, 0.5, true)]))
        .with_input(InputRole::Prompt, Payload::text("   "));
    assert!(read_prompt(&blank).is_ok());
}

/// A prompt that is not text (a picture, a handle) is refused as well, rather than ignored while
/// the clicks are answered.
#[test]
fn a_prompt_that_is_not_text_is_refused() {
    let png = picture(4, 4, 9);
    let not_words = Payload::inline("image/png", png.clone()).unwrap();
    let asked =
        request(png, json!([click(0.5, 0.5, true)])).with_input(InputRole::Prompt, not_words);
    let e = read_prompt(&asked).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert!(e.message.contains("text"), "{}", e.message);
}

/// Clicks outside the picture, missing or mistyped fields, the wrong shape and too many clicks
/// are refused; exactly the most allowed is accepted.
#[test]
fn hostile_numbers_and_shapes_are_refused() {
    let png = picture(4, 4, 9);
    for points in [
        json!([click(1.5, 0.2, true)]),
        json!([click(0.5, -0.1, true)]),
        json!([{"x": 0.5, "y": 0.2}]),
        json!([{"x": "0.5", "y": 0.2, "positive": true}]),
        json!("middle"),
        json!(vec![click(0.1, 0.1, true); MAX_POINTS + 1]),
    ] {
        assert!(
            read_prompt(&request(png.clone(), points.clone())).is_err(),
            "{points}"
        );
    }
    let many = json!(vec![click(0.1, 0.1, true); MAX_POINTS]);
    assert!(read_prompt(&request(png, many)).is_ok());
}

/// The mask is the picture's size, white where the model selected and black elsewhere, and the
/// result says it matched with the model's score.
#[test]
fn the_mask_comes_back_at_the_size_of_the_picture_white_where_selected() {
    let fake = Fake::left_half();
    let mut model = Loaded::new(fake.clone());
    let out = run(
        &mut model,
        &request(picture(40, 20, 100), json!([click(0.2, 0.5, true)])),
    )
    .unwrap();
    let mask = mask_of(&out);
    assert_eq!((mask.width, mask.height), (40, 20));
    let at = |x: usize, y: usize| mask.bytes[(y * mask.width + x) * 3];
    assert!(at(2, 10) > 250, "the left is selected");
    assert!(at(37, 10) < 5, "the right is not");
    let found = segment::read_found(&out).unwrap();
    assert!(found.matched);
    assert_eq!(found.score, Some(0.9));
}

/// The clicks reach the model in order, with their positions and signs unchanged.
#[test]
fn the_clicks_reach_the_model_as_given() {
    let fake = Fake::left_half();
    let mut model = Loaded::new(fake.clone());
    let points = json!([click(0.1, 0.9, true), click(0.7, 0.3, false)]);
    run(&mut model, &request(picture(8, 8, 1), points)).unwrap();
    let asked = fake.asked.lock().unwrap();
    assert_eq!(
        asked[0].0,
        vec![
            Click {
                x: 0.1,
                y: 0.9,
                positive: true
            },
            Click {
                x: 0.7,
                y: 0.3,
                positive: false
            },
        ]
    );
}

/// Three clicks on one picture encode it once; a different picture is encoded again.
#[test]
fn a_picture_is_looked_at_once_however_many_clicks_follow() {
    let fake = Fake::left_half();
    let mut model = Loaded::new(fake.clone());
    let same = picture(16, 16, 50);
    for x in [0.2, 0.4, 0.6] {
        run(
            &mut model,
            &request(same.clone(), json!([click(x, 0.5, true)])),
        )
        .unwrap();
    }
    assert_eq!(fake.encodes.load(Ordering::SeqCst), 1);
    assert_eq!(fake.asked.lock().unwrap().len(), 3);
    run(
        &mut model,
        &request(picture(16, 16, 200), json!([click(0.5, 0.5, true)])),
    )
    .unwrap();
    assert_eq!(
        fake.encodes.load(Ordering::SeqCst),
        2,
        "a different picture is looked at again"
    );
}

/// When the model selects nothing, the result says so, with no score, and the mask is empty at
/// the picture's size.
#[test]
fn nothing_selected_is_said_so_and_the_mask_is_empty() {
    let mut model = Loaded::new(Fake::answering(vec![-8.0; MASK_SIDE * MASK_SIDE]));
    let out = run(
        &mut model,
        &request(picture(10, 6, 7), json!([click(0.5, 0.5, true)])),
    )
    .unwrap();
    let found = segment::read_found(&out).unwrap();
    assert!(!found.matched);
    assert_eq!(found.score, None);
    let mask = mask_of(&out);
    assert_eq!((mask.width, mask.height), (10, 6));
    assert!(mask.bytes.iter().all(|b| *b == 0));
}

/// A link in place of the picture is unsupported and no picture is an invalid request.
#[test]
fn a_link_instead_of_a_picture_and_a_missing_picture_are_refused() {
    let mut model = Loaded::new(Fake::left_half());
    let linked = Request::new(capability())
        .with_input(
            InputRole::Source,
            Payload::handle("https://example.invalid/a.png", "image/png", None).unwrap(),
        )
        .with_param("points", json!([click(0.5, 0.5, true)]));
    assert_eq!(
        run(&mut model, &linked).unwrap_err().kind,
        ErrorKind::Unsupported
    );
    let nothing = Request::new(capability()).with_param("points", json!([click(0.5, 0.5, true)]));
    assert_eq!(
        run(&mut model, &nothing).unwrap_err().kind,
        ErrorKind::InvalidRequest
    );
}

/// A runner built on the stand-in passes `check_runner`, including refusing a request with no
/// picture.
#[test]
fn it_passes_the_shared_runner_checks_end_to_end() {
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
    let good = request(picture(24, 12, 80), json!([click(0.3, 0.5, true)]));
    // No picture: refused when the job is submitted, before any work.
    let bad = Request::new(capability()).with_param("points", json!([click(0.3, 0.5, true)]));
    check_runner(
        Arc::new(FakeRunner(Fake::left_half())),
        &weights,
        &good,
        &bad,
        ErrorKind::InvalidRequest,
    )
    .unwrap();
}

/// Loading refuses an encoder without its decoder beside it, another file name, and files that
/// are not ONNX, each in words that say what is wrong.
#[test]
fn weights_that_are_not_both_files_in_one_folder_say_what_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let encoder = dir.path().join(ENCODER_FILE);
    std::fs::write(&encoder, b"x").unwrap();
    let e = Sam21Runner.load(&encoder).err().unwrap();
    assert_eq!(e.kind, ErrorKind::Unsupported);
    assert!(
        e.message.contains(DECODER_FILE) && e.message.contains("Set up AI"),
        "{}",
        e.message
    );
    // Another file name is not the encoder, even with the decoder beside it.
    std::fs::write(dir.path().join(DECODER_FILE), b"x").unwrap();
    let other = dir.path().join("model.onnx");
    std::fs::write(&other, b"x").unwrap();
    let e = Sam21Runner.load(&other).err().unwrap();
    assert_eq!(e.kind, ErrorKind::Unsupported, "{}", e.message);
    assert!(e.message.contains(ENCODER_FILE), "{}", e.message);
    // Both files, but not ONNX: refused with the model's own words, not a panic.
    let e = Sam21Runner.load(&encoder).err().unwrap();
    assert!(e.message.contains("SAM 2.1"), "{}", e.message);
}

/// A box alone is enough: it reaches the model as given, with no clicks, and a box with clicks
/// keeps both. A box turned round is refused before any work.
#[test]
fn a_box_reaches_the_model_with_or_without_clicks() {
    let fake = Fake::left_half();
    let mut model = Loaded::new(fake.clone());
    let boxed = Area {
        x0: 0.1,
        y0: 0.2,
        x1: 0.6,
        y1: 0.9,
    };
    let source = || {
        Request::new(capability()).with_input(
            InputRole::Source,
            Payload::inline("image/png", picture(8, 8, 1)).unwrap(),
        )
    };
    run(&mut model, &segment::with_area(source(), boxed)).unwrap();
    let with_click = segment::with_area(
        request(picture(8, 8, 1), json!([click(0.3, 0.5, false)])),
        boxed,
    );
    run(&mut model, &with_click).unwrap();
    let asked = fake.asked.lock().unwrap();
    assert_eq!(asked[0], (Vec::new(), Some(boxed)));
    assert_eq!(asked[1].0.len(), 1);
    assert_eq!(asked[1].1, Some(boxed));
    drop(asked);
    let turned = source().with_param("box", json!({"x0": 0.6, "y0": 0.2, "x1": 0.1, "y1": 0.9}));
    assert_eq!(
        run(&mut model, &turned).unwrap_err().kind,
        ErrorKind::InvalidRequest
    );
    assert_eq!(
        fake.encodes.load(Ordering::SeqCst),
        1,
        "refused before looking"
    );
}

/// A score from the model outside 0 to 1 is held to that range before an app reads it.
#[test]
fn a_score_outside_zero_to_one_is_held_to_it() {
    for (given, want) in [(7.0, 1.0), (-3.0, 0.0)] {
        let mut fake = Fake::left_half();
        fake.score = given;
        let out = run(
            &mut Loaded::new(fake),
            &request(picture(8, 8, 1), json!([click(0.5, 0.5, true)])),
        )
        .unwrap();
        assert_eq!(segment::read_found(&out).unwrap().score, Some(want));
    }
}
