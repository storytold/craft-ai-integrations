//! The real LaMa ONNX file through [`LamaRunner`]. Skipped unless `CRAFT_TEST_LAMA_ONNX_DIR`
//! names a folder holding `lama.onnx` (the catalogue's `Carve/LaMa-ONNX` `lama_fp32.onnx` file).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use craft_ai_core::{CapabilityId, ErrorKind, InputRole, Payload, Request};
use craft_ai_engine::testing::check_runner_paced;
use craft_ai_engine::{ModelRunner, RunContext};
use craft_ai_engine_rten::lama::{LAMA_FILE, LamaRunner};
use craft_ai_engine_rten::picture::decode_picture;

fn weights() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("CRAFT_TEST_LAMA_ONNX_DIR")?).join(LAMA_FILE))
}

/// Pale sky backdrop with a dark square object in the center and its matching mask.
fn scene_and_mask(w: u32, h: u32) -> (Vec<u8>, Vec<u8>) {
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    let mut mask = Vec::with_capacity((w * h) as usize);
    let (x0, x1) = (w * 3 / 8, w * 5 / 8);
    let (y0, y1) = (h * 3 / 8, h * 5 / 8);
    for y in 0..h {
        for x in 0..w {
            let inside = x >= x0 && x < x1 && y >= y0 && y < y1;
            rgb.extend_from_slice(if inside {
                &[25, 20, 20]
            } else {
                &[210, 225, 240]
            });
            mask.push(if inside { 255 } else { 0 });
        }
    }
    let encode = |color: png::ColorType, data: &[u8]| {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(color);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(data).unwrap();
        writer.finish().unwrap();
        out
    };
    (
        encode(png::ColorType::Rgb, &rgb),
        encode(png::ColorType::Grayscale, &mask),
    )
}

fn request(w: u32, h: u32) -> Request {
    let (src, mask) = scene_and_mask(w, h);
    Request::new(CapabilityId::parse("image.inpaint").unwrap())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", src).unwrap(),
        )
        .with_input(InputRole::Mask, Payload::inline("image/png", mask).unwrap())
}

#[test]
#[ignore = "needs the real LaMa ONNX file; see the module docs"]
fn the_real_model_removes_the_dark_square_into_the_pale_backdrop() {
    let Some(weights) = weights() else {
        eprintln!("skipped: CRAFT_TEST_LAMA_ONNX_DIR is not set");
        return;
    };
    let mut model = LamaRunner.load(&weights).unwrap();
    let out = model
        .run(&request(512, 512), &RunContext::detached())
        .unwrap();
    let Payload::Inline { data, .. } = &out[0].payload else {
        panic!("expected inline PNG")
    };
    let filled = decode_picture(data).unwrap();
    assert_eq!((filled.width, filled.height), (512, 512));
    let mid = (256 * 512 + 256) * 3;
    assert!(
        filled.bytes[mid] > 150,
        "center remained dark: {:?}",
        &filled.bytes[mid..mid + 3]
    );
    assert_eq!(&filled.bytes[0..3], &[210, 225, 240]);
}

#[test]
#[ignore = "needs the real LaMa ONNX file; see the module docs"]
fn the_real_runner_passes_the_shared_checks() {
    let Some(weights) = weights() else {
        eprintln!("skipped: CRAFT_TEST_LAMA_ONNX_DIR is not set");
        return;
    };
    let good = request(256, 256);
    let bad = Request::new(CapabilityId::parse("image.inpaint").unwrap());
    check_runner_paced(
        Arc::new(LamaRunner),
        &weights,
        &good,
        &bad,
        ErrorKind::InvalidRequest,
        Duration::from_millis(100),
    )
    .unwrap();
}
