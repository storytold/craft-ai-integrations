//! Getting a picture into the network and the network's answer out: resizing so the longer side
//! is 518 px and both sides are multiples of the 14 px patch, the ImageNet normalisation, and the
//! min-max normalisation of the answer to 0..1 (larger is nearer).

use craft_ai_core::AiError;

/// Side of the (square) network input in pixels.
pub(super) const MODEL_SIZE: u32 = 518;
/// Patch size of the DINOv2 backbone: both preprocessed sides are multiples of it.
pub(super) const PATCH: u32 = 14;
/// Largest accepted working image, in pixels (w * h), so lengths and allocations stay bounded.
pub(super) const MAX_PIXELS: u64 = 1 << 28;

/// ImageNet channel means and standard deviations.
pub(super) const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
pub(super) const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// A relative depth map: `data` is `width * height` values in 0..1, min-max normalised, larger =
/// nearer.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct DepthMap {
    pub width: u32,
    pub height: u32,
    pub data: Vec<f32>,
}

/// An input resized and normalised for the network.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Prepared {
    pub width: u32,
    pub height: u32,
    /// Planar (NCHW with N = 1) channels: `3 * width * height` normalised values.
    pub data: Vec<f32>,
}

/// The preprocessed size for a `w` x `h` input: the longer side is [`MODEL_SIZE`], the shorter is
/// scaled to match and rounded to a multiple of [`PATCH`] (at least one patch).
pub(super) fn target_size(w: u32, h: u32) -> Result<(u32, u32), AiError> {
    check_dims(w, h)?;
    let snap = |short: u32, long: u32| -> u32 {
        let v = f64::from(short) * f64::from(MODEL_SIZE) / f64::from(long);
        let p = f64::from(PATCH);
        let n = ((v / p).round() as u32).max(1);
        (n * PATCH).min(MODEL_SIZE)
    };
    Ok(if w >= h {
        (MODEL_SIZE, snap(h, w))
    } else {
        (snap(w, h), MODEL_SIZE)
    })
}

pub(super) fn check_dims(w: u32, h: u32) -> Result<(), AiError> {
    if w == 0 || h == 0 {
        return Err(AiError::invalid("The picture is empty."));
    }
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(AiError::invalid(format!(
            "The picture is too large to estimate depth from ({w} x {h})."
        )));
    }
    Ok(())
}

/// `w` x `h` scaled down, keeping its shape, so the longer side is at most `cap` (each side at
/// least 1). A picture already inside `cap` keeps its size.
pub(super) fn fit_within(w: u32, h: u32, cap: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= cap || long == 0 {
        return (w, h);
    }
    let scale = f64::from(cap) / f64::from(long);
    let side = |v: u32| ((f64::from(v) * scale).round() as u32).max(1);
    (side(w), side(h))
}

/// Weights of a separable triangle resampler: for each output index the first source index and
/// the normalised weights. The kernel is widened by the scale when shrinking, so a downscale
/// averages the covered source pixels (area-correct) and an upscale is plain bilinear.
fn axis_weights(src: usize, dst: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = src as f64 / dst as f64;
    let support = scale.max(1.0);
    (0..dst)
        .map(|o| {
            let centre = (o as f64 + 0.5) * scale;
            let lo = ((centre - support).floor().max(0.0)) as usize;
            let hi = (((centre + support).ceil()) as usize).min(src);
            let mut w: Vec<f32> = (lo..hi)
                .map(|i| (1.0 - ((i as f64 + 0.5 - centre) / support).abs()).max(0.0) as f32)
                .collect();
            let sum: f32 = w.iter().sum();
            if sum > 0.0 {
                w.iter_mut().for_each(|x| *x /= sum);
            } else {
                // Cannot happen for a non-empty range; fall back to the nearest pixel.
                let n = (centre as usize).min(src.saturating_sub(1));
                return (n, vec![1.0]);
            }
            (lo, w)
        })
        .collect()
}

/// Resizes interleaved RGB (`w * h * 3` values of any numeric type) to `nw` x `nh`, multiplying
/// every value by `scale` (255 turns 8-bit values into 0..1). A shorter slice than the sizes say
/// reads as zeros past its end rather than failing.
pub(super) fn resize_rgb<T: Copy + Into<f32>>(
    src: &[T],
    (w, h): (usize, usize),
    (nw, nh): (usize, usize),
    scale: f32,
) -> Vec<f32> {
    let (wx, wy) = (axis_weights(w, nw), axis_weights(h, nh));
    // Horizontal pass: h rows x nw columns x 3.
    let mut mid = vec![0.0f32; h * nw * 3];
    for y in 0..h {
        let row = src.get(y * w * 3..(y + 1) * w * 3).unwrap_or(&[]);
        for (x, (start, ws)) in wx.iter().enumerate() {
            let mut acc = [0.0f32; 3];
            for (k, wt) in ws.iter().enumerate() {
                let i = (start + k) * 3;
                if let Some(px) = row.get(i..i + 3) {
                    for (a, p) in acc.iter_mut().zip(px) {
                        *a += (*p).into() * wt;
                    }
                }
            }
            if let Some(o) = mid.get_mut((y * nw + x) * 3..(y * nw + x) * 3 + 3) {
                for (o, a) in o.iter_mut().zip(acc) {
                    *o = a * scale;
                }
            }
        }
    }
    // Vertical pass.
    let mut out = vec![0.0f32; nh * nw * 3];
    for (y, (start, ws)) in wy.iter().enumerate() {
        for x in 0..nw {
            let mut acc = [0.0f32; 3];
            for (k, wt) in ws.iter().enumerate() {
                let i = ((start + k) * nw + x) * 3;
                if let Some(px) = mid.get(i..i + 3) {
                    for (a, p) in acc.iter_mut().zip(px) {
                        *a += p * wt;
                    }
                }
            }
            if let Some(o) = out.get_mut((y * nw + x) * 3..(y * nw + x) * 3 + 3) {
                o.copy_from_slice(&acc);
            }
        }
    }
    out
}

/// Resizes interleaved RGB (0..1, `w * h * 3`) to [`target_size`] and normalises with the ImageNet
/// mean and standard deviation into planar channels.
pub(super) fn preprocess(rgb: &[f32], w: u32, h: u32) -> Result<Prepared, AiError> {
    let (nw, nh) = target_size(w, h)?;
    let (wu, hu) = (w as usize, h as usize);
    if rgb.len() != wu * hu * 3 {
        return Err(AiError::invalid(format!(
            "expected {} values, got {}",
            wu * hu * 3,
            rgb.len()
        )));
    }
    if rgb.iter().any(|v| !v.is_finite()) {
        return Err(AiError::invalid("The picture has a non-finite value."));
    }
    let resized = resize_rgb(rgb, (wu, hu), (nw as usize, nh as usize), 1.0);
    let plane = nw as usize * nh as usize;
    let mut data = vec![0.0f32; 3 * plane];
    for (i, px) in resized.as_chunks::<3>().0.iter().enumerate() {
        for (c, v) in px.iter().enumerate() {
            if let Some(o) = data.get_mut(c * plane + i) {
                *o = (v - MEAN[c]) / STD[c];
            }
        }
    }
    Ok(Prepared {
        width: nw,
        height: nh,
        data,
    })
}

/// Turns the network's raw output (`width * height` values, larger = nearer) into a [`DepthMap`]
/// min-max normalised to 0..1. A constant map becomes all zeros.
pub(super) fn postprocess(raw: &[f32], width: u32, height: u32) -> Result<DepthMap, AiError> {
    check_dims(width, height)?;
    if raw.len() != width as usize * height as usize {
        return Err(AiError::invalid("depth output has the wrong size"));
    }
    if raw.iter().any(|v| !v.is_finite()) {
        return Err(super::backend("The depth output is not finite."));
    }
    let (lo, hi) = raw
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &v| {
            (a.min(v), b.max(v))
        });
    let range = hi - lo;
    let data = if range > f32::EPSILON {
        raw.iter()
            .map(|v| ((v - lo) / range).clamp(0.0, 1.0))
            .collect()
    } else {
        vec![0.0; raw.len()]
    };
    Ok(DepthMap {
        width,
        height,
        data,
    })
}

#[cfg(test)]
mod tests;
