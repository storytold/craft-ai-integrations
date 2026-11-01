//! The real DeepFilterNet 3 files through the real runner. Skipped unless
//! `CRAFT_TEST_DFN3_ONNX_DIR` names a folder holding `enc.onnx`, `erb_dec.onnx` and `df_dec.onnx`.
//! `CRAFT_TEST_DFN3_WAV` may also name a 48 kHz WAV of someone talking in a noisy room.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use craft_ai_core::{CapabilityId, ErrorKind, InputRole, Payload, Request};
use craft_ai_engine::testing::check_runner_paced;
use craft_ai_engine::{ModelRunner, RunContext};
use craft_ai_engine_rten::{DFN3_ENCODER_FILE, Dfn3Runner, WAV};

fn weights() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("CRAFT_TEST_DFN3_ONNX_DIR")?).join(DFN3_ENCODER_FILE))
}

/// Mono 32-bit float WAV bytes at 48 kHz.
fn wav(samples: &[f32]) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut out = Cursor::new(Vec::new());
    let mut w = hound::WavWriter::new(&mut out, spec).unwrap();
    samples.iter().for_each(|s| w.write_sample(*s).unwrap());
    w.finalize().unwrap();
    out.into_inner()
}

/// All channels of a WAV, one after the other, and its channel count.
fn read(data: &[u8]) -> (Vec<f32>, u16, u32) {
    let r = hound::WavReader::new(Cursor::new(data)).unwrap();
    let spec = r.spec();
    let samples = match spec.sample_format {
        hound::SampleFormat::Float => r.into_samples::<f32>().map(Result::unwrap).collect(),
        hound::SampleFormat::Int => {
            let scale = (1u64 << (spec.bits_per_sample - 1)) as f32;
            r.into_samples::<i32>()
                .map(|s| s.unwrap() as f32 / scale)
                .collect()
        }
    };
    (samples, spec.channels, spec.sample_rate)
}

fn request(data: Vec<u8>) -> Request {
    Request::new(CapabilityId::parse("audio.enhance").unwrap())
        .with_input(InputRole::Source, Payload::inline(WAV, data).unwrap())
}

fn enhanced(req: &Request) -> Vec<u8> {
    let weights = weights().unwrap();
    let out = Dfn3Runner
        .load(&weights)
        .unwrap()
        .run(req, &RunContext::detached())
        .unwrap();
    let Payload::Inline { data, .. } = &out[0].payload else {
        panic!("inline")
    };
    data.clone()
}

fn rms_db(x: &[f32]) -> f32 {
    let p = x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32;
    10. * (p + 1e-20).log10()
}

/// Steady noise, a deterministic stand-in for hiss.
fn hiss(len: usize, level: f32) -> Vec<f32> {
    let mut state = 7u32;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            level * ((state >> 8) as f32 / (1u32 << 24) as f32 - 0.5)
        })
        .collect()
}

/// The real files pass the shared runner checks: a whole job, Cancel, a refused request, an
/// unknown job, missing weights and a second job with the weights reused.
#[test]
#[ignore = "needs the real DeepFilterNet 3 files; see the module docs"]
fn the_real_model_passes_the_shared_runner_checks() {
    let Some(weights) = weights() else {
        eprintln!("skipped: CRAFT_TEST_DFN3_ONNX_DIR is not set");
        return;
    };
    let good = request(wav(&hiss(96_000, 0.05)));
    let bad = Request::new(CapabilityId::parse("audio.enhance").unwrap());
    check_runner_paced(
        Arc::new(Dfn3Runner),
        &weights,
        &good,
        &bad,
        ErrorKind::InvalidRequest,
        Duration::from_millis(20),
    )
    .unwrap();
}

/// Hiss with nobody talking is taken down by more than 30 dB, and the result is as long as the
/// input.
#[test]
#[ignore = "needs the real DeepFilterNet 3 files; see the module docs"]
fn the_real_model_takes_hiss_with_no_voice_down_by_more_than_30_db() {
    if weights().is_none() {
        eprintln!("skipped: CRAFT_TEST_DFN3_ONNX_DIR is not set");
        return;
    }
    let x = hiss(2 * 48_000, 0.05);
    let (y, channels, rate) = read(&enhanced(&request(wav(&x))));
    assert_eq!((channels, rate, y.len()), (1, 48_000, x.len()));
    let drop = rms_db(&x) - rms_db(&y);
    eprintln!("hiss reduced by {drop:.1} dB");
    assert!(drop > 30., "only {drop:.1} dB");
}

/// A real recording keeps its voice: the same length, finite, the overall level within 3 dB, and
/// the quietest 10 % of 50 ms windows (the room between words) at least 20 dB quieter.
#[test]
#[ignore = "needs the real DeepFilterNet 3 files and a recording; see the module docs"]
fn a_real_recording_keeps_the_voice_and_loses_the_room() {
    let (Some(_), Some(path)) = (weights(), std::env::var_os("CRAFT_TEST_DFN3_WAV")) else {
        eprintln!("skipped: CRAFT_TEST_DFN3_ONNX_DIR and CRAFT_TEST_DFN3_WAV are not set");
        return;
    };
    let data = std::fs::read(path).unwrap();
    let (x, channels, _) = read(&data);
    let (y, out_channels, _) = read(&enhanced(&request(data)));
    assert_eq!((out_channels, y.len()), (channels, x.len()));
    assert!(y.iter().all(|s| s.is_finite()));
    let quiet_floor = |s: &[f32]| {
        let mut w: Vec<f32> = s
            .chunks(2_400 * usize::from(channels))
            .map(rms_db)
            .collect();
        w.sort_by(f32::total_cmp);
        w[w.len() / 10]
    };
    let level = rms_db(&x) - rms_db(&y);
    let floor = quiet_floor(&x) - quiet_floor(&y);
    eprintln!("overall level change {level:.1} dB, room between words down {floor:.1} dB");
    assert!(level.abs() < 3., "{level:.1} dB");
    assert!(floor > 20., "{floor:.1} dB");
}
