//! The signal processing around the three DeepFilterNet 3 graphs, ported from the publisher's own
//! Rust (`libDF`, MIT OR Apache-2.0) and its offline `df.enhance.enhance`:
//!
//! - **analysis / synthesis**: a 960-point real FFT every 480 samples (10 ms at 48 kHz) under the
//!   Vorbis window, scaled on analysis only, overlap-added on synthesis;
//! - **features**: per frame, the power of 32 ERB bands in dB with an exponential mean
//!   normalisation, and the lowest 96 bins with an exponential unit normalisation;
//! - **enhancement**: the ERB gains on every bin, then a five-tap complex filter over frames
//!   `t - 2 ..= t + 2` for the lowest 96 bins (the deep filter), then an optional blend with the
//!   noisy spectrum that caps the reduction.
//!
//! Nothing here touches the model files, so every step is tested on its own.

use std::f64::consts::PI;

use realfft::RealFftPlanner;
use realfft::num_complex::Complex32;

/// Sample rate the model was trained at.
pub const SAMPLE_RATE: u32 = 48_000;
pub(crate) const FFT: usize = 960;
pub(crate) const HOP: usize = 480;
/// Frequency bins of one frame.
pub(crate) const FREQS: usize = FFT / 2 + 1;
pub(crate) const NB_ERB: usize = 32;
/// Bins the deep filter works on (0 to 4.8 kHz).
pub(crate) const NB_DF: usize = 96;
const MIN_ERB_FREQS: usize = 2;
pub(crate) const DF_ORDER: usize = 5;
const DF_LOOKAHEAD: usize = 2;
/// Frames the encoder looks ahead: its features are shifted this far.
pub(crate) const CONV_LOOKAHEAD: usize = 2;
/// `calc_norm_alpha(48000, 480, tau = 1)`, rounded as libDF rounds it.
const ALPHA: f32 = 0.99;

/// The model's view of one channel: the noisy spectrum and the two feature sets.
pub(crate) struct Analysed {
    pub spec: Vec<Vec<Complex32>>,
    /// `[frames, NB_ERB]`, already shifted by [`CONV_LOOKAHEAD`].
    pub erb: Vec<f32>,
    /// `[2, frames, NB_DF]` (real parts, then imaginary parts), shifted the same way.
    pub cplx: Vec<f32>,
}

/// What the graphs say about one channel.
pub(crate) struct Estimate {
    /// `[frames, NB_ERB]` gains.
    pub gains: Vec<f32>,
    /// `[frames, NB_DF, DF_ORDER, 2]` filter taps.
    pub coefs: Vec<f32>,
}

fn freq2erb(hz: f32) -> f32 {
    9.265 * (hz / (24.7 * 9.265)).ln_1p()
}

fn erb2freq(erb: f32) -> f32 {
    24.7 * 9.265 * ((erb / 9.265).exp() - 1.)
}

/// Bins per ERB band, low to high; they add up to [`FREQS`].
pub(crate) fn erb_widths() -> Vec<usize> {
    let bin_hz = SAMPLE_RATE as f32 / FFT as f32;
    let (low, high) = (freq2erb(0.), freq2erb((SAMPLE_RATE / 2) as f32));
    let step = (high - low) / NB_ERB as f32;
    let mut widths = vec![0usize; NB_ERB];
    let (mut prev, mut over) = (0i64, 0i64);
    for (i, w) in widths.iter_mut().enumerate() {
        let edge = (erb2freq(low + (i + 1) as f32 * step) / bin_hz).round() as i64;
        let mut n = edge - prev - over;
        if n < MIN_ERB_FREQS as i64 {
            over = MIN_ERB_FREQS as i64 - n;
            n = MIN_ERB_FREQS as i64;
        } else {
            over = 0;
        }
        *w = n.max(0) as usize;
        prev = edge;
    }
    let total: usize = widths.iter().sum::<usize>() + 1;
    if let Some(last) = widths.last_mut() {
        *last += 1;
        *last -= total.saturating_sub(FREQS);
    }
    widths
}

/// The Vorbis window, `sin(π/2 · sin²(π(n + ½)/N))`.
fn window() -> Vec<f32> {
    let half = (FFT / 2) as f64;
    (0..FFT)
        .map(|i| {
            let s = (0.5 * PI * (i as f64 + 0.5) / half).sin();
            (0.5 * PI * s * s).sin() as f32
        })
        .collect()
}

/// The spectrum of `x`, one frame per [`HOP`] samples (a last partial hop is dropped).
pub(crate) fn analysis(x: &[f32]) -> Vec<Vec<Complex32>> {
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FFT);
    let window = window();
    let norm = (2 * HOP) as f32 / (FFT * FFT) as f32;
    let mut previous = vec![0f32; FFT - HOP];
    let mut buf = vec![0f32; FFT];
    let mut frames = Vec::with_capacity(x.len() / HOP);
    for chunk in x.as_chunks::<HOP>().0 {
        for ((b, s), w) in buf
            .iter_mut()
            .zip(previous.iter().chain(chunk))
            .zip(&window)
        {
            *b = s * w;
        }
        previous.copy_from_slice(chunk);
        let mut out = vec![Complex32::default(); FREQS];
        // Sizes come from the plan itself, so this cannot fail; a zero frame is the safe answer.
        if fft.process(&mut buf, &mut out).is_err() {
            out.fill(Complex32::default());
        }
        out.iter_mut().for_each(|c| *c *= norm);
        frames.push(out);
    }
    frames
}

/// Samples back from frames made by [`analysis`], [`HOP`] per frame, delayed by `FFT - HOP`.
pub(crate) fn synthesis(spec: &mut [Vec<Complex32>]) -> Vec<f32> {
    let ifft = RealFftPlanner::<f32>::new().plan_fft_inverse(FFT);
    let window = window();
    let mut tail = vec![0f32; FFT - HOP];
    let mut buf = vec![0f32; FFT];
    let mut out = Vec::with_capacity(spec.len() * HOP);
    for frame in spec.iter_mut() {
        // A real signal's DC and Nyquist bins have no imaginary part.
        if let Some(c) = frame.first_mut() {
            c.im = 0.;
        }
        if let Some(c) = frame.last_mut() {
            c.im = 0.;
        }
        if ifft.process(frame, &mut buf).is_err() {
            buf.fill(0.);
        }
        for (b, w) in buf.iter_mut().zip(&window) {
            *b *= w;
        }
        let (now, later) = buf.split_at(HOP);
        out.extend(now.iter().zip(&tail).map(|(a, t)| a + t));
        tail.copy_from_slice(later);
    }
    out
}

/// One channel's spectrum and the features the encoder reads (libDF's `erb`, `erb_norm` and
/// `unit_norm`, with their states starting where libDF starts them).
pub(crate) fn analyse(x: &[f32], widths: &[usize]) -> Analysed {
    let spec = analysis(x);
    let frames = spec.len();
    let mut erb = vec![0f32; frames * NB_ERB];
    let mut cplx = vec![0f32; 2 * frames * NB_DF];
    let mut mean: Vec<f32> = (0..NB_ERB)
        .map(|i| -60. - 30. * i as f32 / (NB_ERB - 1) as f32)
        .collect();
    let mut unit: Vec<f32> = (0..NB_DF)
        .map(|i| 0.001 - 0.0009 * i as f32 / (NB_DF - 1) as f32)
        .collect();
    // Frame `t` lands at row `t - CONV_LOOKAHEAD`: the encoder sees two frames ahead, and the
    // last rows stay zero (the model's `pad_feat`).
    for (t, frame) in spec.iter().enumerate() {
        let row = t.checked_sub(CONV_LOOKAHEAD);
        let mut start = 0;
        for (band, (&w, state)) in widths.iter().zip(mean.iter_mut()).enumerate() {
            let bins = frame.get(start..start + w).unwrap_or_default();
            start += w;
            let power = bins.iter().map(|c| c.norm_sqr()).sum::<f32>() / w.max(1) as f32;
            let db = (power + 1e-10).log10() * 10.;
            *state = db * (1. - ALPHA) + *state * ALPHA;
            if let Some(r) = row {
                erb[r * NB_ERB + band] = (db - *state) / 40.;
            }
        }
        for (f, (c, state)) in frame.iter().zip(unit.iter_mut()).enumerate() {
            *state = c.norm() * (1. - ALPHA) + *state * ALPHA;
            if let Some(r) = row {
                let d = state.sqrt();
                cplx[r * NB_DF + f] = c.re / d;
                cplx[(frames + r) * NB_DF + f] = c.im / d;
            }
        }
    }
    Analysed { spec, erb, cplx }
}

/// Applies the model's estimate to the noisy spectrum. `keep` (0 to 1) is how much of the noisy
/// spectrum is mixed back in, which caps the reduction at `-20·log10(keep)` dB.
pub(crate) fn enhance(
    spec: &[Vec<Complex32>],
    est: &Estimate,
    widths: &[usize],
    keep: f32,
) -> Vec<Vec<Complex32>> {
    let frames = spec.len();
    let mut out = Vec::with_capacity(frames);
    for (t, noisy) in spec.iter().enumerate() {
        let mut frame = vec![Complex32::default(); FREQS];
        let mut start = 0;
        for (band, &w) in widths.iter().enumerate() {
            let g = est.gains.get(t * NB_ERB + band).copied().unwrap_or(0.);
            for f in start..(start + w).min(FREQS) {
                frame[f] = noisy[f] * g;
            }
            start += w;
        }
        for (f, slot) in frame.iter_mut().enumerate().take(NB_DF) {
            let mut acc = Complex32::default();
            for k in 0..DF_ORDER {
                let Some(src) = (t + k).checked_sub(DF_ORDER - 1 - DF_LOOKAHEAD) else {
                    continue;
                };
                let (Some(s), Some(c)) = (
                    spec.get(src).and_then(|s| s.get(f)),
                    est.coefs.get(((t * NB_DF + f) * DF_ORDER + k) * 2..),
                ) else {
                    continue;
                };
                if let [re, im, ..] = c {
                    acc += s * Complex32::new(*re, *im);
                }
            }
            *slot = acc;
        }
        if keep > 0. {
            for (o, n) in frame.iter_mut().zip(noisy) {
                *o = *o * (1. - keep) + n * keep;
            }
        }
        out.push(frame);
    }
    out
}

/// The whole offline path for one channel, as `df.enhance.enhance` with `pad = True`: pad by one
/// FFT, analyse, estimate, enhance, synthesise, and cut the STFT delay back off, so the result is
/// exactly as long as `x` and lined up with it.
pub(crate) fn process(
    x: &[f32],
    widths: &[usize],
    keep: f32,
    estimate: impl FnOnce(&Analysed) -> Result<Estimate, craft_ai_core::AiError>,
) -> Result<Vec<f32>, craft_ai_core::AiError> {
    let mut padded = Vec::with_capacity(x.len() + FFT);
    padded.extend_from_slice(x);
    padded.resize(x.len() + FFT, 0.);
    let analysed = analyse(&padded, widths);
    let est = estimate(&analysed)?;
    let mut spec = enhance(&analysed.spec, &est, widths, keep);
    let y = synthesis(&mut spec);
    let delay = FFT - HOP;
    Ok(y.get(delay..delay + x.len())
        .map(<[f32]>::to_vec)
        .unwrap_or_default())
}
