//! The real BEN2 + ViTMatte files through the real runner. Skipped unless
//! `CRAFT_TEST_MATTE_ONNX_DIR` names a folder holding `ben2.onnx`, `vitmatte-small.onnx` and
//! `vitmatte-base.onnx` (the catalogue's files).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use craft_ai_core::{CapabilityId, ErrorKind, InputRole, Payload, Request};
use craft_ai_engine::testing::check_runner_paced;
use craft_ai_engine::{ModelRunner, RunContext};
use craft_ai_engine_rten::matte::{BEN2_FILE, MatteRunner};

fn weights() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("CRAFT_TEST_MATTE_ONNX_DIR")?).join(BEN2_FILE))
}

/// A `w` × `h` PNG: a red disc on a pale grey backdrop.
fn disc_png(w: u32, h: u32) -> Vec<u8> {
    let (cx, cy, r) = (w as f32 / 2.0, h as f32 / 2.0, h as f32 / 4.0);
    let mut pixels = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            pixels.extend_from_slice(if dx * dx + dy * dy < r * r {
                &[204, 26, 26]
            } else {
                &[235, 235, 230]
            });
        }
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(&pixels).unwrap();
    }
    out
}

fn request(png: Vec<u8>) -> Request {
    Request::new(CapabilityId::parse("image.matte").unwrap()).with_input(
        InputRole::Source,
        Payload::inline("image/png", png).unwrap(),
    )
}

/// The disc is matted, the backdrop is not, at the picture's size.
#[test]
#[ignore = "needs the real BEN2 + ViTMatte files; see the module docs"]
fn the_real_models_cut_the_disc_out() {
    let Some(weights) = weights() else {
        eprintln!("skipped: CRAFT_TEST_MATTE_ONNX_DIR is not set");
        return;
    };
    let mut model = MatteRunner.load(&weights).unwrap();
    let started = std::time::Instant::now();
    let out = model
        .run(&request(disc_png(1600, 1200)), &RunContext::detached())
        .unwrap();
    eprintln!("cut out in {:?}", started.elapsed());
    let Payload::Inline { data, .. } = &out[0].payload else {
        panic!("an inline matte is expected")
    };
    let mut reader = png::Decoder::new(std::io::Cursor::new(data))
        .read_info()
        .unwrap();
    let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut buffer).unwrap();
    assert_eq!((frame.width, frame.height), (1600, 1200));
    assert!(
        buffer[600 * 1600 + 800] > 240,
        "centre {}",
        buffer[600 * 1600 + 800]
    );
    assert!(
        buffer[20 * 1600 + 20] < 15,
        "corner {}",
        buffer[20 * 1600 + 20]
    );
}

/// The shared conformance suite with the real files: a job's whole life, Cancel, a refused
/// request, a second job.
#[test]
#[ignore = "needs the real BEN2 + ViTMatte files; see the module docs"]
fn the_real_runner_passes_the_shared_checks() {
    let Some(weights) = weights() else {
        eprintln!("skipped: CRAFT_TEST_MATTE_ONNX_DIR is not set");
        return;
    };
    let good = request(disc_png(320, 240));
    let bad = Request::new(CapabilityId::parse("image.matte").unwrap());
    check_runner_paced(
        Arc::new(MatteRunner),
        &weights,
        &good,
        &bad,
        ErrorKind::InvalidRequest,
        Duration::from_millis(100),
    )
    .unwrap();
}
