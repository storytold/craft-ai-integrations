//! The real SAM 2.1 files through the real Burn runner, compared with the rten runner on the
//! published ONNX files. Skipped unless the files are named (and run with `--release`, as a debug
//! build of the processor engines is far too slow):
//!
//! - `CRAFT_TEST_SAM21_ONNX_DIR`: a folder holding `encoder.onnx` and `decoder.onnx` (rten reads
//!   these as they are).
//! - `CRAFT_TEST_SAM21_BURN_DIR`: the folder `craft-ai-burn-setup` saved the prepared copy in
//!   (`encoder.bpk` and `decoder.bpk`). Make it once with
//!   `cargo run -p craft-ai-burn-setup --features conversion --example prepare_sam21 -- <onnx dir> <cache dir>`
//!   and use the folder it prints.
//! - `CRAFT_TEST_SAM21_IMAGE`: the astronaut sample, a 512 square PNG.
//!
//! The comparison is the mask a person is handed (white above half), because that is what the
//! engines are meant to agree on.

use std::path::PathBuf;
use std::time::Instant;

use craft_ai_core::segment::{self, Area};
use craft_ai_core::{CapabilityId, InputRole, Output, Payload, Request};
use craft_ai_engine::{LoadedModel, ModelRunner, RunContext};
use serde_json::json;

/// The files and picture named in the environment, or `None` when any is not set.
fn files() -> Option<(PathBuf, PathBuf, Vec<u8>)> {
    let onnx = PathBuf::from(std::env::var_os("CRAFT_TEST_SAM21_ONNX_DIR")?);
    let burn = PathBuf::from(std::env::var_os("CRAFT_TEST_SAM21_BURN_DIR")?);
    let image = std::fs::read(std::env::var_os("CRAFT_TEST_SAM21_IMAGE")?).ok()?;
    Some((onnx, burn, image))
}

/// An `image.segment` request for `image` with one click on the object at (`x`, `y`) of 512.
fn click(image: &[u8], x: f64, y: f64) -> Request {
    Request::new(CapabilityId::parse("image.segment").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", image.to_vec()).unwrap(),
        )
        .with_param(
            "points",
            json!([{"x": x / 512.0, "y": y / 512.0, "positive": true}]),
        )
}

/// The mask of an answer as one flag per pixel (white above half), with its size.
fn mask(out: &[Output]) -> (Vec<bool>, (u32, u32)) {
    let Payload::Inline { data, .. } = &out[0].payload else {
        panic!("an inline mask is expected");
    };
    let mut reader = png::Decoder::new(std::io::Cursor::new(data))
        .read_info()
        .unwrap();
    let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut buffer).unwrap();
    let flags = buffer[..frame.buffer_size()].iter().map(|b| *b > 127);
    (flags.collect(), (frame.width, frame.height))
}

/// Intersection over union of two equally sized masks.
fn iou(a: &[bool], b: &[bool]) -> f64 {
    assert_eq!(a.len(), b.len());
    let both = a.iter().zip(b).filter(|(x, y)| **x && **y).count();
    let either = a.iter().zip(b).filter(|(x, y)| **x || **y).count();
    both as f64 / either.max(1) as f64
}

/// Runs a request and says how long it took, in seconds.
fn timed(model: &mut Box<dyn LoadedModel>, request: &Request) -> (Vec<Output>, f64) {
    let start = Instant::now();
    let out = model.run(request, &RunContext::detached()).unwrap();
    (out, start.elapsed().as_secs_f64())
}

/// One click on the suit: the prepared Burn weights select what the ONNX files select under rten
/// (best-mask IoU at least 0.95), a second click on the same picture is quick, and a box works.
#[test]
#[ignore = "needs the real SAM 2.1 files; see the module docs"]
fn burn_selects_what_rten_selects_with_one_click() {
    let Some((onnx, burn, image)) = files() else {
        eprintln!("skipped: the three CRAFT_TEST_SAM21_* variables are not set");
        return;
    };
    eprintln!(
        "graphics card answers: {}",
        craft_ai_engine_burn::graphics_card_answers()
    );

    let start = Instant::now();
    let mut reference = craft_ai_engine_rten::Sam21Runner
        .load(&onnx.join(craft_ai_engine_rten::ENCODER_FILE))
        .unwrap();
    eprintln!("rten load: {:.2} s", start.elapsed().as_secs_f64());
    let start = Instant::now();
    let mut model = craft_ai_engine_burn::Sam21Runner
        .load(&burn.join(craft_ai_engine_burn::ENCODER_FILE))
        .unwrap();
    eprintln!("burn load: {:.2} s", start.elapsed().as_secs_f64());

    let first = click(&image, 150.0, 330.0);
    let (want, rten_s) = timed(&mut reference, &first);
    let (got, burn_first_s) = timed(&mut model, &first);
    let (want_mask, size) = mask(&want);
    let (got_mask, got_size) = mask(&got);
    assert_eq!(size, got_size, "the mask is the size of the picture");
    let overlap = iou(&want_mask, &got_mask);
    let share = |m: &[bool]| 100.0 * m.iter().filter(|b| **b).count() as f64 / m.len() as f64;
    let (found_burn, found_rten) = (
        segment::read_found(&got).unwrap(),
        segment::read_found(&want).unwrap(),
    );
    eprintln!(
        "one click: IoU {overlap:.4}; selected {:.1} % (burn) vs {:.1} % (rten); score {:?} vs {:?}",
        share(&got_mask),
        share(&want_mask),
        found_burn.score,
        found_rten.score
    );
    eprintln!(
        "first run: rten {rten_s:.2} s, burn {burn_first_s:.2} s (burn includes shader set-up)"
    );
    assert!(found_burn.matched && found_rten.matched);
    assert!(overlap >= 0.95, "best-mask IoU {overlap} is below 0.95");

    // The picture is kept: the second click on it costs only the decoder.
    let second = click(&image, 160.0, 340.0);
    let (_, rten_second_s) = timed(&mut reference, &second);
    let (second_out, burn_second_s) = timed(&mut model, &second);
    eprintln!("second click, same picture: rten {rten_second_s:.3} s, burn {burn_second_s:.3} s");
    assert!(segment::read_found(&second_out).unwrap().matched);
    assert!(burn_second_s < 2.0, "a second click took {burn_second_s} s");

    // A box around the suit selects it too, and agrees with rten.
    let area = Area {
        x0: 90.0 / 512.0,
        y0: 190.0 / 512.0,
        x1: 400.0 / 512.0,
        y1: 500.0 / 512.0,
    };
    let boxed = segment::with_area(
        Request::new(CapabilityId::parse("image.segment").unwrap()).with_input(
            InputRole::Source,
            Payload::inline("image/png", image.clone()).unwrap(),
        ),
        area,
    );
    let (want_box, _) = timed(&mut reference, &boxed);
    let (got_box, _) = timed(&mut model, &boxed);
    let box_overlap = iou(&mask(&want_box).0, &mask(&got_box).0);
    eprintln!("box: IoU {box_overlap:.4}");
    assert!(box_overlap >= 0.95, "box IoU {box_overlap} is below 0.95");
}

/// A truncated copy of the real prepared file is refused with an error, not a crash.
#[test]
#[ignore = "needs the real SAM 2.1 files; see the module docs"]
fn a_truncated_real_weights_file_is_an_error() {
    let Some((_, burn, _)) = files() else {
        eprintln!("skipped: the three CRAFT_TEST_SAM21_* variables are not set");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    for name in [
        craft_ai_engine_burn::ENCODER_FILE,
        craft_ai_engine_burn::DECODER_FILE,
    ] {
        let bytes = std::fs::read(burn.join(name)).unwrap();
        std::fs::write(dir.path().join(name), &bytes[..bytes.len() / 2]).unwrap();
    }
    let error = craft_ai_engine_burn::Sam21Runner
        .load(&dir.path().join(craft_ai_engine_burn::ENCODER_FILE))
        .err()
        .unwrap();
    assert!(error.message.contains("Set up AI"), "{}", error.message);
}

/// Loading from the downloaded files prepares the saved copy beside the model folder, once: the
/// first load converts, the second only checks hashes, and the copy the runner reads is the one in
/// `burn-prepared`. Needs the converter (`prepare`) and a release build, so it is ignored by default.
#[cfg(feature = "prepare")]
#[test]
#[ignore = "needs the real SAM 2.1 files; see the module docs"]
fn loading_the_downloaded_files_prepares_once_beside_the_model_folder() {
    let Some(onnx) = std::env::var_os("CRAFT_TEST_SAM21_ONNX_DIR").map(PathBuf::from) else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let models = root.path().join("models");
    std::fs::create_dir_all(&models).unwrap();
    for name in ["encoder.onnx", "decoder.onnx"] {
        std::fs::copy(onnx.join(name), models.join(name)).unwrap();
    }
    let started = Instant::now();
    craft_ai_engine_burn::Sam21Runner
        .load(&models.join("encoder.onnx"))
        .unwrap();
    let first = started.elapsed();
    let saved = root.path().join("burn-prepared");
    // The saved copy sits in a folder named for the source files and converter revision.
    let copies: Vec<_> = std::fs::read_dir(&saved)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("encoder.bpk").is_file() && p.join("decoder.bpk").is_file())
        .collect();
    assert_eq!(copies.len(), 1, "one saved copy in {saved:?}");
    assert!(
        !models.join("encoder.bpk").exists(),
        "nothing is written into the model folder"
    );
    let started = Instant::now();
    craft_ai_engine_burn::Sam21Runner
        .load(&models.join("encoder.onnx"))
        .unwrap();
    eprintln!("prepare+load {first:?}, reuse+load {:?}", started.elapsed());
}
