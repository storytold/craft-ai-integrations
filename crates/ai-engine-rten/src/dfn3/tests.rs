use std::io::Cursor;
use std::sync::{Arc, Mutex};

use craft_ai_core::{CapabilityId, ErrorKind, InputRole, Payload, Request};
use craft_ai_engine::RunContext;
use craft_ai_engine::testing::check_runner;
use serde_json::json;

use super::dsp::{CONV_LOOKAHEAD, FREQS};
use super::*;

fn capability() -> CapabilityId {
    CapabilityId::parse("audio.enhance").unwrap()
}

/// What the stand-in was shown: frames, ERB features and complex features.
type Seen = Arc<Mutex<Vec<(usize, Vec<f32>, Vec<f32>)>>>;

/// A stand-in for the model with fixed gains and a fixed centre tap, recording what it was shown.
#[derive(Clone)]
struct Fake {
    gain: f32,
    tap: f32,
    seen: Seen,
}

impl Fake {
    /// Gains of one and a centre tap of one: the spectrum passes through untouched.
    fn identity() -> Self {
        Self::with(1., 1.)
    }
    /// Everything turned off: silence.
    fn silence() -> Self {
        Self::with(0., 0.)
    }
    fn with(gain: f32, tap: f32) -> Self {
        Self {
            gain,
            tap,
            seen: Arc::default(),
        }
    }
}

impl Estimator for Fake {
    fn estimate(&self, a: &Analysed) -> Result<Estimate, AiError> {
        let frames = a.spec.len();
        self.seen
            .lock()
            .unwrap()
            .push((frames, a.erb.clone(), a.cplx.clone()));
        let mut coefs = vec![0f32; frames * NB_DF * DF_ORDER * 2];
        // tap k = 2 reads frame t itself (taps cover t - 2 ..= t + 2)
        for cell in coefs.as_chunks_mut::<{ DF_ORDER * 2 }>().0 {
            cell[4] = self.tap;
        }
        Ok(Estimate {
            gains: vec![self.gain; frames * NB_ERB],
            coefs,
        })
    }
}

/// A deterministic, noise-like test signal with a tone in it.
fn signal(len: usize, seed: u32) -> Vec<f32> {
    let mut state = seed;
    (0..len)
        .map(|i| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            0.3 * noise + 0.4 * (i as f32 * 0.031).sin()
        })
        .collect()
}

/// `channels` as a WAV in the given format.
fn wav_of(rate: u32, channels: &[Vec<f32>], bits: u16, float: bool) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: channels.len() as u16,
        sample_rate: rate,
        bits_per_sample: bits,
        sample_format: if float {
            hound::SampleFormat::Float
        } else {
            hound::SampleFormat::Int
        },
    };
    let mut out = Cursor::new(Vec::new());
    let mut w = hound::WavWriter::new(&mut out, spec).unwrap();
    for i in 0..channels[0].len() {
        for ch in channels {
            if float {
                w.write_sample(ch[i]).unwrap();
            } else {
                let scale = (1i64 << (bits - 1)) as f32 - 1.;
                w.write_sample((ch[i] * scale).round() as i32).unwrap();
            }
        }
    }
    w.finalize().unwrap();
    out.into_inner()
}

fn request(data: Vec<u8>) -> Request {
    Request::new(capability()).with_input(InputRole::Source, Payload::inline(WAV, data).unwrap())
}

fn run(model: &mut Enhancer<Fake>, req: &Request) -> Result<wav::Audio, AiError> {
    let out = model.run(req, &RunContext::detached())?;
    assert_eq!(out.len(), 1);
    let Payload::Inline { media_type, data } = &out[0].payload else {
        panic!("not inline")
    };
    assert_eq!(media_type.as_str(), WAV);
    wav::decode(data)
}

fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0., f32::max)
}

#[test]
fn the_erb_bands_cover_every_bin_with_at_least_two_each() {
    let w = dsp::erb_widths();
    assert_eq!(w.len(), NB_ERB);
    assert_eq!(w.iter().sum::<usize>(), FREQS);
    assert!(w.iter().all(|&n| n >= 2), "{w:?}");
    // narrow at the bottom, wide at the top, as on the ERB scale
    assert_eq!(w[0], 2);
    assert!(w[NB_ERB - 1] > 30, "{w:?}");
}

/// The strongest check of the plumbing: with gains of one and the centre tap at one, the audio
/// comes back as it went in, the same length and lined up sample for sample. A wrong window,
/// scale, frame shift, tap index or delay all break this.
#[test]
fn a_pass_through_model_gives_back_the_input_exactly_aligned() {
    let x = signal(48_000 + 123, 1);
    let y = dsp::process(&x, &dsp::erb_widths(), 0., |a| Fake::identity().estimate(a)).unwrap();
    assert_eq!(y.len(), x.len());
    let d = max_diff(&x, &y);
    assert!(d < 1e-4, "max difference {d}");
}

#[test]
fn a_model_that_removes_everything_gives_silence_and_the_cap_keeps_some() {
    let x = signal(9_600, 2);
    let widths = dsp::erb_widths();
    let y = dsp::process(&x, &widths, 0., |a| Fake::silence().estimate(a)).unwrap();
    assert!(y.iter().all(|s| s.abs() < 1e-6));
    // capped at 20 dB, a tenth of the original stays
    let keep = 10f32.powf(-20. / 20.);
    let y = dsp::process(&x, &widths, keep, |a| Fake::silence().estimate(a)).unwrap();
    let expected: Vec<f32> = x.iter().map(|s| s * keep).collect();
    assert!(max_diff(&expected, &y) < 1e-4);
}

/// The encoder looks two frames ahead: frame `t`'s features sit at row `t - 2`, and the last two
/// rows are zero, as the model's own `pad_feat` leaves them.
#[test]
fn the_features_are_shifted_by_the_encoder_lookahead() {
    let fake = Fake::identity();
    dsp::process(&signal(4_800, 3), &dsp::erb_widths(), 0., |a| {
        fake.estimate(a)
    })
    .unwrap();
    let seen = fake.seen.lock().unwrap();
    let (frames, erb, cplx) = &seen[0];
    assert_eq!(erb.len(), frames * NB_ERB);
    assert_eq!(cplx.len(), 2 * frames * NB_DF);
    let rows = |v: &[f32], width: usize, from: usize| -> bool {
        v[from * width..(from + CONV_LOOKAHEAD) * width]
            .iter()
            .all(|x| *x == 0.)
    };
    assert!(rows(erb, NB_ERB, frames - CONV_LOOKAHEAD));
    assert!(rows(
        &cplx[..frames * NB_DF],
        NB_DF,
        frames - CONV_LOOKAHEAD
    ));
    assert!(rows(
        &cplx[frames * NB_DF..],
        NB_DF,
        frames - CONV_LOOKAHEAD
    ));
    assert!(
        erb[..NB_ERB].iter().any(|x| *x != 0.),
        "row 0 holds frame 2"
    );
}

#[test]
fn the_runner_is_the_rten_deepfilternet3_the_catalogue_names_and_needs_a_recording() {
    assert_eq!(Dfn3Runner.engine(), "rten");
    let info = Dfn3Runner.model();
    assert_eq!(info.id.as_str(), DFN3_MODEL);
    let spec = info
        .capabilities
        .iter()
        .find(|c| c.capability == capability())
        .expect("audio.enhance");
    assert!(spec.requires.contains(&InputRole::Source));
    assert!(
        crate::runners()
            .iter()
            .any(|r| r.model().id.as_str() == DFN3_MODEL)
    );
}

#[test]
fn stereo_comes_back_the_same_rate_channels_and_length_each_channel_on_its_own() {
    let (l, r) = (signal(30_000, 4), signal(30_000, 5));
    let fake = Fake::identity();
    let mut model = Enhancer::new(fake.clone());
    let out = run(
        &mut model,
        &request(wav_of(48_000, &[l.clone(), r.clone()], 32, true)),
    )
    .unwrap();
    assert_eq!(out.sample_rate, 48_000);
    assert_eq!(out.channels.len(), 2);
    assert!(max_diff(&out.channels[0], &l) < 1e-4);
    assert!(max_diff(&out.channels[1], &r) < 1e-4);
    assert_eq!(
        fake.seen.lock().unwrap().len(),
        2,
        "one model run per channel"
    );
}

#[test]
fn pcm_input_is_read_at_its_scale() {
    let x = signal(9_600, 6);
    for bits in [16, 24, 32] {
        let mut model = Enhancer::new(Fake::identity());
        let out = run(
            &mut model,
            &request(wav_of(48_000, std::slice::from_ref(&x), bits, false)),
        )
        .unwrap();
        let d = max_diff(&out.channels[0], &x);
        assert!(d < 1e-3, "{bits}-bit: {d}");
    }
}

#[test]
fn max_reduction_db_caps_the_reduction() {
    let x = signal(9_600, 7);
    let mut model = Enhancer::new(Fake::silence());
    let req = request(wav_of(48_000, std::slice::from_ref(&x), 32, true))
        .with_param(MAX_REDUCTION_DB, json!(6.0));
    let out = run(&mut model, &req).unwrap();
    let keep = 10f32.powf(-6. / 20.);
    let expected: Vec<f32> = x.iter().map(|s| s * keep).collect();
    assert!(max_diff(&out.channels[0], &expected) < 1e-4);
    // a blend, not an addition: when the model keeps everything, a cap changes nothing
    let mut pass = Enhancer::new(Fake::identity());
    let req = request(wav_of(48_000, std::slice::from_ref(&x), 32, true))
        .with_param(MAX_REDUCTION_DB, json!(6.0));
    assert!(max_diff(&run(&mut pass, &req).unwrap().channels[0], &x) < 1e-4);
    // 0 dB: nothing is taken away
    let req = request(wav_of(48_000, std::slice::from_ref(&x), 32, true))
        .with_param(MAX_REDUCTION_DB, json!(0));
    assert!(max_diff(&run(&mut model, &req).unwrap().channels[0], &x) < 1e-4);
}

#[test]
fn bad_requests_are_refused_in_plain_words() {
    let mut model = Enhancer::new(Fake::identity());
    let ok = wav_of(48_000, &[signal(4_800, 8)], 16, false);
    let refused = |model: &mut Enhancer<Fake>, req: Request, words: &str| {
        let e = model.run(&req, &RunContext::detached()).unwrap_err();
        assert!(e.message.contains(words), "{}", e.message);
        e.kind
    };
    // no recording at all
    assert_eq!(
        refused(
            &mut model,
            Request::new(capability()),
            "recording is needed"
        ),
        ErrorKind::InvalidRequest
    );
    // not a WAV
    let mp3 = Request::new(capability()).with_input(
        InputRole::Source,
        Payload::inline("audio/mpeg", vec![0xff, 0xfb, 0, 0]).unwrap(),
    );
    refused(&mut model, mp3, "WAV");
    refused(&mut model, request(b"RIFF....not a wav".to_vec()), "WAV");
    // a link instead of the audio
    let link = Request::new(capability()).with_input(
        InputRole::Source,
        Payload::handle("file:///a.wav", WAV, None).unwrap(),
    );
    assert_eq!(
        refused(&mut model, link, "not a link"),
        ErrorKind::Unsupported
    );
    // the wrong rate
    refused(
        &mut model,
        request(wav_of(44_100, &[signal(4_410, 9)], 16, false)),
        "48 kHz",
    );
    // out-of-range or non-numeric caps
    for bad in [json!(-1), json!(101), json!("loud"), json!(null)] {
        let req = request(ok.clone()).with_param(MAX_REDUCTION_DB, bad);
        refused(&mut model, req, "max_reduction_db");
    }
}

/// The answer's size is worked out exactly before any processing: the prediction matches what is
/// written, for any channel count and length.
#[test]
fn the_answer_size_is_predicted_exactly() {
    for (channels, frames) in [(1, 0), (1, 4_801), (2, 30_000), (8, 7)] {
        let data = wav::encode(48_000, &vec![vec![0.25; frames]; channels]).unwrap();
        assert_eq!(
            wav::encoded_len(48_000, channels, frames).unwrap(),
            data.len() as u64
        );
    }
}

/// A 16-bit recording small enough to send (18 MB) whose 32-bit answer (36 MB) would be over the
/// shared inline limit is refused in words before the model runs, not answered with a result that
/// breaks the limit.
#[test]
fn a_recording_whose_answer_would_be_over_the_inline_limit_is_refused_before_processing() {
    let limit = craft_ai_core::Limits::default().max_inline_bytes as usize;
    let frames = limit / 2 / 2 + 48_000; // 16-bit mono: just over half the limit as float
    let input = wav_of(48_000, &[vec![0.1; frames]], 16, false);
    assert!(input.len() < limit, "the request itself fits");
    let fake = Fake::identity();
    let mut model = Enhancer::new(fake.clone());
    let e = model
        .run(&request(input), &RunContext::detached())
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::InvalidRequest);
    assert!(e.message.contains("over the 32 MB"), "{}", e.message);
    assert!(
        fake.seen.lock().unwrap().is_empty(),
        "refused before the model ran"
    );
    // the same length minus a little fits and is answered
    let fits = (limit - 1_024) / 4;
    let mut model = Enhancer::new(Fake::silence());
    let out = model
        .run(
            &request(wav_of(48_000, &[vec![0.1; fits]], 16, false)),
            &RunContext::detached(),
        )
        .unwrap();
    let Payload::Inline { data, .. } = &out[0].payload else {
        panic!()
    };
    assert!(data.len() <= limit);
}

/// The encoded answer is checked against the limit again before it is returned.
#[test]
fn an_encoded_answer_over_the_limit_is_never_returned() {
    let small = craft_ai_core::Limits {
        max_inline_bytes: 100,
        ..Default::default()
    };
    assert!(answer(vec![0; 100], &small).is_ok());
    let e = answer(vec![0; 101], &small).unwrap_err();
    assert_eq!(e.kind, ErrorKind::InvalidRequest);
    assert!(
        e.message.contains("cleaned recording would be"),
        "{}",
        e.message
    );
}

/// A runner built on the stand-in passes the shared runner checks: a whole job, Cancel, a refused
/// request, an unknown job, missing weights and a second job.
#[test]
fn it_passes_the_shared_runner_checks_end_to_end() {
    struct FakeRunner;
    impl ModelRunner for FakeRunner {
        fn engine(&self) -> &str {
            Dfn3Runner.engine()
        }
        fn model(&self) -> ModelInfo {
            Dfn3Runner.model()
        }
        fn load(&self, weights: &Path) -> Result<Box<dyn LoadedModel>, AiError> {
            std::fs::metadata(weights).map_err(|_| AiError::unsupported("missing"))?;
            Ok(Box::new(Enhancer::new(Fake::identity())))
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let weights = dir.path().join(DFN3_ENCODER_FILE);
    std::fs::write(&weights, b"weights").unwrap();
    let good = request(wav_of(48_000, &[signal(48_000, 10)], 16, false));
    let bad = Request::new(capability());
    check_runner(
        Arc::new(FakeRunner),
        &weights,
        &good,
        &bad,
        ErrorKind::InvalidRequest,
    )
    .unwrap();
}

#[test]
fn files_that_are_not_all_three_in_one_folder_say_what_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let enc = dir.path().join(DFN3_ENCODER_FILE);
    std::fs::write(&enc, b"x").unwrap();
    std::fs::write(dir.path().join(DFN3_ERB_DECODER_FILE), b"x").unwrap();
    let e = Dfn3Runner.load(&enc).err().expect("df_dec.onnx is missing");
    assert!(
        e.message.contains("enc.onnx, erb_dec.onnx and df_dec.onnx"),
        "{}",
        e.message
    );
    // all three present but not ONNX: refused as unloadable, not a crash
    std::fs::write(dir.path().join(DFN3_DF_DECODER_FILE), b"x").unwrap();
    assert!(Dfn3Runner.load(&enc).is_err());
    // the weights file must be the encoder
    let e = Dfn3Runner
        .load(&dir.path().join(DFN3_DF_DECODER_FILE))
        .err()
        .unwrap();
    assert_eq!(e.kind, ErrorKind::Unsupported);
}
