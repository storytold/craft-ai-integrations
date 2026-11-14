//! The real SAM 2.1 files through the real runner. Skipped unless the files are named:
//! `CRAFT_TEST_SAM21_ONNX_DIR` (a folder holding `encoder.onnx` and `decoder.onnx`) and
//! `CRAFT_TEST_SAM21_IMAGE` (the astronaut sample, a 512 square PNG).
//!
//! Reference: ONNX Runtime 1.28 on the same picture with one click at (150, 330) of 512 puts
//! 22.46 % of the 256 mask in the selection, and the best-mask IoU against this runner was 1.0.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use craft_ai_core::{CapabilityId, ErrorKind, InputRole, Payload, Request};
use craft_ai_engine::testing::check_runner_paced;
use craft_ai_engine::{ModelRunner, RunContext};
use craft_ai_engine_rten::{ENCODER_FILE, Sam21Runner};
use serde_json::json;

/// The encoder path and the sample picture named in the environment, or `None` when they are not
/// set.
fn files() -> Option<(PathBuf, Vec<u8>)> {
    let dir = PathBuf::from(std::env::var_os("CRAFT_TEST_SAM21_ONNX_DIR")?);
    let image = std::fs::read(std::env::var_os("CRAFT_TEST_SAM21_IMAGE")?).ok()?;
    Some((dir.join(ENCODER_FILE), image))
}

/// An `image.segment` request for `image` with one click on the object at (`x`, `y`).
fn request(image: &[u8], x: f64, y: f64) -> Request {
    Request::new(CapabilityId::parse("image.segment").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", image.to_vec()).unwrap(),
        )
        .with_param("points", json!([{"x": x, "y": y, "positive": true}]))
}

/// The real files select 21 to 24 percent of the sample at its own size, and a second click on the
/// same picture takes under two seconds.
#[test]
#[ignore = "needs the real SAM 2.1 files; see the module docs"]
fn the_real_model_selects_the_suit_at_the_size_of_the_picture() {
    let Some((weights, image)) = files() else {
        eprintln!("skipped: CRAFT_TEST_SAM21_ONNX_DIR and CRAFT_TEST_SAM21_IMAGE are not set");
        return;
    };
    let mut model = Sam21Runner.load(&weights).unwrap();
    let out = model
        .run(
            &request(&image, 150.0 / 512.0, 330.0 / 512.0),
            &RunContext::detached(),
        )
        .unwrap();
    let Payload::Inline { data, .. } = &out[0].payload else {
        panic!("an inline mask is expected");
    };
    let mut reader = png::Decoder::new(std::io::Cursor::new(data))
        .read_info()
        .unwrap();
    let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut buffer).unwrap();
    assert_eq!((frame.width, frame.height), (512, 512));
    let selected = buffer.iter().filter(|v| **v > 127).count() as f64 / (512.0 * 512.0);
    assert!((0.21..0.24).contains(&selected), "selected {selected}");
    // A second click on the same picture reuses what the encoder saw.
    let started = std::time::Instant::now();
    model
        .run(
            &request(&image, 400.0 / 512.0, 300.0 / 512.0),
            &RunContext::detached(),
        )
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

/// The real runner passes the shared runner checks, at the pace a person would accept.
#[test]
#[ignore = "needs the real SAM 2.1 files; see the module docs"]
fn the_real_runner_passes_the_shared_runner_checks() {
    let Some((weights, image)) = files() else {
        eprintln!("skipped: CRAFT_TEST_SAM21_ONNX_DIR and CRAFT_TEST_SAM21_IMAGE are not set");
        return;
    };
    let bad = Request::new(CapabilityId::parse("image.segment").unwrap())
        .with_param("points", json!([{"x": 0.5, "y": 0.5, "positive": true}]));
    check_runner_paced(
        Arc::new(Sam21Runner),
        &weights,
        &request(&image, 0.3, 0.6),
        &bad,
        ErrorKind::InvalidRequest,
        Duration::from_millis(50),
    )
    .unwrap();
}

/// The 512 square mask in `out`, one byte per pixel.
fn mask_bytes(out: &[craft_ai_core::Output]) -> Vec<u8> {
    let Some(Payload::Inline { data, .. }) = craft_ai_core::segment::mask(out) else {
        panic!("an inline mask is expected");
    };
    let mut reader = png::Decoder::new(std::io::Cursor::new(data))
        .read_info()
        .unwrap();
    let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
    reader.next_frame(&mut buffer).unwrap();
    buffer
}

/// A box drawn round what one click selected selects much the same thing (overlap above 0.8),
/// and the result says it matched.
#[test]
#[ignore = "needs the real SAM 2.1 files; see the module docs"]
fn a_box_round_the_suit_selects_what_a_click_on_it_does() {
    let Some((weights, image)) = files() else {
        eprintln!("skipped: CRAFT_TEST_SAM21_ONNX_DIR and CRAFT_TEST_SAM21_IMAGE are not set");
        return;
    };
    let mut model = Sam21Runner.load(&weights).unwrap();
    let clicked = mask_bytes(
        &model
            .run(
                &request(&image, 150.0 / 512.0, 330.0 / 512.0),
                &RunContext::detached(),
            )
            .unwrap(),
    );
    let side = 512usize;
    let on: Vec<(usize, usize)> = (0..side * side)
        .filter(|i| clicked[*i] > 127)
        .map(|i| (i % side, i / side))
        .collect();
    let edge = |f: fn(&(usize, usize)) -> usize, max: bool| {
        let it = on.iter().map(f);
        let v = if max { it.max() } else { it.min() };
        v.unwrap() as f32 / side as f32
    };
    let area = craft_ai_core::segment::Area {
        x0: edge(|p| p.0, false),
        y0: edge(|p| p.1, false),
        x1: edge(|p| p.0 + 1, true),
        y1: edge(|p| p.1 + 1, true),
    };
    let boxed_request = craft_ai_core::segment::with_area(
        Request::new(CapabilityId::parse("image.segment").unwrap()).with_input(
            InputRole::Source,
            Payload::inline("image/png", image.clone()).unwrap(),
        ),
        area,
    );
    let out = model.run(&boxed_request, &RunContext::detached()).unwrap();
    assert!(craft_ai_core::segment::read_found(&out).unwrap().matched);
    let boxed = mask_bytes(&out);
    let both = (0..side * side)
        .filter(|i| clicked[*i] > 127 && boxed[*i] > 127)
        .count();
    let either = (0..side * side)
        .filter(|i| clicked[*i] > 127 || boxed[*i] > 127)
        .count();
    let overlap = both as f64 / either as f64;
    println!("box {area:?}, overlap with the click's mask {overlap:.3}");
    assert!(overlap > 0.8, "overlap {overlap}");
}
