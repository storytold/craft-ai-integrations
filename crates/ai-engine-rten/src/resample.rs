//! Getting a picture into the model and a mask out of it. Plain arithmetic, no model needed.
//!
//! SAM 2.1 reads a 1024 by 1024 picture: the source stretched to that square (the shape is not
//! kept), each channel scaled to 0..1 and then normalised with the ImageNet mean and deviation.
//! Its mask comes back 256 by 256 over the same stretched square, so it is resampled straight to
//! the source size. This is the preparation the published ONNX export documents.

use craft_ai_core::AiError;

/// The side of the square the model reads.
pub const SIDE: usize = 1024;
/// The side of the mask the model writes.
pub const MASK_SIDE: usize = 256;

const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Weights for resampling `src` samples to `dst` with a triangle filter whose width grows with the
/// reduction (an antialiased bilinear, the way Pillow's `BILINEAR` and most photo tools do it).
/// Each entry is the first source index and its weights, which sum to one.
fn weights(src: usize, dst: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = src as f64 / dst as f64;
    let width = scale.max(1.0);
    (0..dst)
        .map(|o| {
            let centre = (o as f64 + 0.5) * scale;
            let lo = ((centre - width + 0.5).floor().max(0.0)) as usize;
            let hi = ((centre + width + 0.5).floor() as usize).min(src);
            let mut w: Vec<f32> = (lo..hi)
                .map(|i| (1.0 - ((i as f64 - centre + 0.5) / width).abs()).max(0.0) as f32)
                .collect();
            let sum: f32 = w.iter().sum();
            if sum > 0.0 {
                w.iter_mut().for_each(|v| *v /= sum);
            }
            (lo, w)
        })
        .collect()
}

/// An 8-bit RGB picture (`w` by `h`, row by row) as the model's input: `[3, 1024, 1024]` floats,
/// channel first.
pub fn preprocess(rgb: &[u8], w: usize, h: usize) -> Result<Vec<f32>, AiError> {
    let enough = w
        .checked_mul(h)
        .and_then(|n| n.checked_mul(3))
        .is_some_and(|n| n > 0 && rgb.len() >= n);
    if !enough {
        return Err(AiError::invalid(
            "The picture does not have the pixels it says it has.",
        ));
    }
    let (wx, wy) = (weights(w, SIDE), weights(h, SIDE));
    // Horizontal pass: h rows of SIDE columns, three channels each.
    let mut rows = vec![0f32; h * SIDE * 3];
    for y in 0..h {
        for (ox, (lo, ws)) in wx.iter().enumerate() {
            let mut acc = [0f32; 3];
            for (k, wt) in ws.iter().enumerate() {
                let at = (y * w + lo + k) * 3;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += wt * f32::from(rgb.get(at + c).copied().unwrap_or(0));
                }
            }
            let to = (y * SIDE + ox) * 3;
            if let Some(dst) = rows.get_mut(to..to + 3) {
                dst.copy_from_slice(&acc);
            }
        }
    }
    // Vertical pass, written channel first. The export was made from 8-bit pictures, so each
    // value is rounded to 8 bits before it is scaled, as an image library would hand it over.
    let mut out = vec![0f32; 3 * SIDE * SIDE];
    for (oy, (lo, ws)) in wy.iter().enumerate() {
        for ox in 0..SIDE {
            let mut acc = [0f32; 3];
            for (k, wt) in ws.iter().enumerate() {
                let at = ((lo + k) * SIDE + ox) * 3;
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += wt * rows.get(at + c).copied().unwrap_or(0.0);
                }
            }
            for (c, a) in acc.iter().enumerate() {
                let value = a.round().clamp(0.0, 255.0) / 255.0;
                if let Some(d) = out.get_mut((c * SIDE + oy) * SIDE + ox) {
                    *d = (value - MEAN[c]) / STD[c];
                }
            }
        }
    }
    Ok(out)
}

/// Mask `logits` (`MASK_SIDE` squared) resampled bilinearly, with half-pixel centres, to `w` by
/// `h` and turned into 0..1 coverage. Anything that is not a full mask gives an empty one.
pub fn coverage(logits: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0f32; w * h];
    if logits.len() < MASK_SIDE * MASK_SIDE {
        return out;
    }
    let at = |x: usize, y: usize| {
        logits
            .get(y.min(MASK_SIDE - 1) * MASK_SIDE + x.min(MASK_SIDE - 1))
            .copied()
            .unwrap_or(-20.0)
    };
    let (sx, sy) = (
        MASK_SIDE as f32 / w.max(1) as f32,
        MASK_SIDE as f32 / h.max(1) as f32,
    );
    for y in 0..h {
        let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        for x in 0..w {
            let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let top = at(x0, y0) * (1.0 - tx) + at(x0 + 1, y0) * tx;
            let bottom = at(x0, y0 + 1) * (1.0 - tx) + at(x0 + 1, y0 + 1) * tx;
            let logit = top * (1.0 - ty) + bottom * ty;
            if let Some(o) = out.get_mut(y * w + x) {
                *o = 1.0 / (1.0 + (-logit).exp());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A white picture of any size becomes one constant value per channel, the mean and deviation
    /// applied.
    #[test]
    fn a_flat_picture_stays_flat_and_is_normalised_per_channel() {
        let rgb = vec![255u8; 30 * 20 * 3];
        let out = preprocess(&rgb, 30, 20).unwrap();
        assert_eq!(out.len(), 3 * SIDE * SIDE);
        for c in 0..3 {
            let want = (1.0 - MEAN[c]) / STD[c];
            let got = &out[c * SIDE * SIDE..(c + 1) * SIDE * SIDE];
            assert!(got.iter().all(|v| (v - want).abs() < 1e-4), "channel {c}");
        }
    }

    /// A wide picture with a dark left half and a bright right half keeps both halves across the
    /// whole square.
    #[test]
    fn the_picture_is_stretched_not_cropped() {
        // Left half black, right half white, in a wide picture: the model's left half is dark and
        // its right half is bright whatever the shape of the source.
        let (w, h) = (40usize, 10usize);
        let mut rgb = Vec::new();
        for _ in 0..h {
            for x in 0..w {
                rgb.extend_from_slice(if x < w / 2 {
                    &[0, 0, 0]
                } else {
                    &[255, 255, 255]
                });
            }
        }
        let out = preprocess(&rgb, w, h).unwrap();
        let row = SIDE / 2;
        assert!(out[row * SIDE + 20] < -1.0);
        assert!(out[row * SIDE + SIDE - 20] > 1.5);
    }

    /// Too few bytes, none, or a size that overflows give an error.
    #[test]
    fn a_short_or_empty_buffer_is_an_error_not_a_panic() {
        assert!(preprocess(&[0; 5], 2, 2).is_err());
        assert!(preprocess(&[], 0, 0).is_err());
        assert!(preprocess(&[0; 12], usize::MAX, 2).is_err());
    }

    /// A mask selected on the left half gives high coverage on the left and low on the right, at
    /// the source size.
    #[test]
    fn coverage_follows_the_logits_at_the_size_of_the_source() {
        // Left half selected, right half not.
        let logits: Vec<f32> = (0..MASK_SIDE * MASK_SIDE)
            .map(|i| {
                if i % MASK_SIDE < MASK_SIDE / 2 {
                    12.0
                } else {
                    -12.0
                }
            })
            .collect();
        let a = coverage(&logits, 8, 2);
        assert_eq!(a.len(), 16);
        assert!(a[0] > 0.99 && a[7] < 0.01);
    }

    /// No logits, or too few, give zero coverage everywhere.
    #[test]
    fn anything_but_a_full_mask_gives_an_empty_one() {
        assert!(coverage(&[], 3, 3).iter().all(|v| *v == 0.0));
        assert!(coverage(&[5.0; 10], 3, 3).iter().all(|v| *v == 0.0));
    }
}
