//! The real Depth Anything V2 Small, through the real runner.
//!
//! Needs the two weight files, so it skips (and says so) unless `CRAFT_DEPTH_WEIGHTS` names the
//! folder that holds `depth_anything_v2_vits.fp16.safetensors` and `dinov2_vits14.fp16.safetensors`.
//! Run it in release mode and read the timings it prints:
//! `CRAFT_DEPTH_WEIGHTS=<folder> cargo test -p craft-ai-engine-candle --release --test depth_live -- --nocapture`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use craft_ai_core::{CapabilityId, ErrorKind, InputRole, Payload, Request};
use craft_ai_engine::testing::check_runner_paced;
use craft_ai_engine::{ModelRunner, RunContext};
use craft_ai_engine_candle::{BACKBONE_FILE, DepthRunner, HEAD_FILE};

const SIZE: (u32, u32) = (1600, 1200);

/// A gradient ground with a bright disc, as an 8-bit RGB PNG.
fn picture() -> Vec<u8> {
    let (w, h) = SIZE;
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 - 800.0, y as f32 - 600.0);
            let v = if dx * dx + dy * dy < 250.0 * 250.0 {
                0.9
            } else {
                0.2 + 0.4 * x as f32 / w as f32
            };
            rgb.extend([
                (v * 255.0) as u8,
                (v * 0.8 * 255.0) as u8,
                ((1.0 - v) * 255.0) as u8,
            ]);
        }
    }
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, w, h);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    let mut writer = e.write_header().unwrap();
    writer.write_image_data(&rgb).unwrap();
    writer.finish().unwrap();
    out
}

/// A run's one output as (width, height, 16-bit values).
fn read_map(out: &[craft_ai_core::Output]) -> (u32, u32, Vec<u16>) {
    assert_eq!(out.len(), 1);
    let Payload::Inline { media_type, data } = &out[0].payload else {
        panic!("the map should come back inline");
    };
    assert_eq!(media_type.as_str(), "image/png");
    let mut reader = png::Decoder::new(std::io::Cursor::new(data))
        .read_info()
        .unwrap();
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut buf).unwrap();
    assert_eq!(
        (frame.color_type, frame.bit_depth),
        (png::ColorType::Grayscale, png::BitDepth::Sixteen)
    );
    let values = buf[..frame.buffer_size()]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes(*c))
        .collect();
    (frame.width, frame.height, values)
}

#[test]
fn the_real_model_estimates_depth_at_both_qualities() {
    let Some(dir) = std::env::var_os("CRAFT_DEPTH_WEIGHTS").map(PathBuf::from) else {
        eprintln!("skipped: set CRAFT_DEPTH_WEIGHTS to the folder holding both depth weight files");
        return;
    };
    assert!(
        dir.join(BACKBONE_FILE).is_file(),
        "{BACKBONE_FILE} is not in the folder"
    );
    let t = Instant::now();
    let mut model = DepthRunner.load(&dir.join(HEAD_FILE)).unwrap();
    eprintln!("load: {:?}", t.elapsed());

    let request = Request::new(CapabilityId::parse("image.depth").unwrap()).with_input(
        InputRole::Source,
        Payload::inline("image/png", picture()).unwrap(),
    );
    for quality in ["fast", "high"] {
        let request = request
            .clone()
            .with_param("quality", serde_json::json!(quality));
        let t = Instant::now();
        let out = model.run(&request, &RunContext::detached()).unwrap();
        eprintln!("{quality}: {:?}", t.elapsed());
        let (w, h, values) = read_map(&out);
        assert_eq!((w, h), SIZE, "{quality}: the map is at the picture's size");
        let (lo, hi) = values
            .iter()
            .fold((u16::MAX, 0), |(a, b), &v| (a.min(v), b.max(v)));
        assert_eq!(
            (lo, hi),
            (0, 65_535),
            "{quality}: stretched to the full range"
        );
        // The disc and the ground are different distances: the map is not flat.
        let at = |x: usize, y: usize| i32::from(values[y * w as usize + x]);
        assert!(
            (at(800, 600) - at(100, 100)).abs() > 3000,
            "{quality}: disc {} vs ground {}",
            at(800, 600),
            at(100, 100)
        );
    }
}

/// The real model behind the real provider, through the shared runner checks (a job's whole life,
/// Cancel, a refused request, missing weights, a second job). A small picture at `fast` quality
/// keeps the several jobs it runs short.
#[test]
fn the_real_model_passes_the_shared_runner_checks() {
    let Some(dir) = std::env::var_os("CRAFT_DEPTH_WEIGHTS").map(PathBuf::from) else {
        eprintln!("skipped: set CRAFT_DEPTH_WEIGHTS to the folder holding both depth weight files");
        return;
    };
    let capability = CapabilityId::parse("image.depth").unwrap();
    let small = {
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, 96, 64);
        e.set_color(png::ColorType::Grayscale);
        e.set_depth(png::BitDepth::Eight);
        let mut writer = e.write_header().unwrap();
        let pixels: Vec<u8> = (0..96 * 64).map(|i| (i % 96 * 2) as u8).collect();
        writer.write_image_data(&pixels).unwrap();
        writer.finish().unwrap();
        out
    };
    let good = Request::new(capability.clone())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", small).unwrap(),
        )
        .with_param("quality", serde_json::json!("fast"));
    // No picture: refused when the job is submitted, before any work.
    let bad = Request::new(capability);
    check_runner_paced(
        Arc::new(DepthRunner),
        &dir.join(HEAD_FILE),
        &good,
        &bad,
        ErrorKind::InvalidRequest,
        Duration::from_millis(50),
    )
    .unwrap();
}
