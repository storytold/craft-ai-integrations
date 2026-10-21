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

/// Whether the horizontal pass runs first. It does for a picture at least as wide as it is tall,
/// so the buffer between the passes is the shorter side times [`SIDE`], never the longer.
fn horizontal_first(w: usize, h: usize) -> bool {
    h <= w
}

/// The floats in the buffer between the two passes: the shorter side of the picture by [`SIDE`],
/// three channels. For a picture of at most 100 megapixels the shorter side is at most 10,000, so
/// this is at most about 123 MB.
fn middle_len(w: usize, h: usize) -> Option<usize> {
    let shorter = if horizontal_first(w, h) { h } else { w };
    shorter.checked_mul(SIDE)?.checked_mul(3)
}

/// A zeroed buffer of `len` floats, or a plain error when that much memory cannot be had, so a
/// hostile size never aborts the process.
fn zeroed(len: Option<usize>) -> Result<Vec<f32>, AiError> {
    let too_big = || AiError::invalid("The picture is too large to select from.");
    let len = len.ok_or_else(too_big)?;
    let mut v = Vec::new();
    v.try_reserve_exact(len).map_err(|_| too_big())?;
    v.resize(len, 0.0);
    Ok(v)
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
    let mut middle = zeroed(middle_len(w, h))?;
    let mut out = zeroed(Some(3 * SIDE * SIDE))?;
    // The export was made from 8-bit pictures, so each value is rounded to 8 bits before it is
    // scaled, as an image library would hand it over. The result is written channel first.
    let mut put = |oy: usize, ox: usize, acc: [f32; 3]| {
        for (c, a) in acc.iter().enumerate() {
            let value = a.round().clamp(0.0, 255.0) / 255.0;
            if let Some(d) = out.get_mut((c * SIDE + oy) * SIDE + ox) {
                *d = (value - MEAN[c]) / STD[c];
            }
        }
    };
    if horizontal_first(w, h) {
        // Horizontal pass: h rows of SIDE columns, three channels each.
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
                if let Some(dst) = middle.get_mut(to..to + 3) {
                    dst.copy_from_slice(&acc);
                }
            }
        }
        // Vertical pass.
        for (oy, (lo, ws)) in wy.iter().enumerate() {
            for ox in 0..SIDE {
                let mut acc = [0f32; 3];
                for (k, wt) in ws.iter().enumerate() {
                    let at = ((lo + k) * SIDE + ox) * 3;
                    for (c, a) in acc.iter_mut().enumerate() {
                        *a += wt * middle.get(at + c).copied().unwrap_or(0.0);
                    }
                }
                put(oy, ox, acc);
            }
        }
    } else {
        // Vertical pass: SIDE rows of w columns, three channels each.
        for (oy, (lo, ws)) in wy.iter().enumerate() {
            for x in 0..w {
                let mut acc = [0f32; 3];
                for (k, wt) in ws.iter().enumerate() {
                    let at = ((lo + k) * w + x) * 3;
                    for (c, a) in acc.iter_mut().enumerate() {
                        *a += wt * f32::from(rgb.get(at + c).copied().unwrap_or(0));
                    }
                }
                let to = (oy * w + x) * 3;
                if let Some(dst) = middle.get_mut(to..to + 3) {
                    dst.copy_from_slice(&acc);
                }
            }
        }
        // Horizontal pass.
        for oy in 0..SIDE {
            for (ox, (lo, ws)) in wx.iter().enumerate() {
                let mut acc = [0f32; 3];
                for (k, wt) in ws.iter().enumerate() {
                    let at = (oy * w + lo + k) * 3;
                    for (c, a) in acc.iter_mut().enumerate() {
                        *a += wt * middle.get(at + c).copied().unwrap_or(0.0);
                    }
                }
                put(oy, ox, acc);
            }
        }
    }
    Ok(out)
}

/// Mask `logits` (`MASK_SIDE` squared) resampled bilinearly, with half-pixel centres, to `w` by
/// `h` and turned into 0..1 coverage. Anything that is not a full mask gives an empty one. A size
/// that overflows, or that cannot be allocated, is an error rather than an abort.
pub fn coverage(logits: &[f32], w: usize, h: usize) -> Result<Vec<f32>, AiError> {
    let mut out = zeroed(w.checked_mul(h))?;
    if logits.len() < MASK_SIDE * MASK_SIDE {
        return Ok(out);
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
    Ok(out)
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

    /// The buffer between the two passes is the shorter side by `SIDE`, whichever way the picture
    /// is turned, so a one pixel wide picture of 100 million rows cannot ask for terabytes.
    #[test]
    fn the_middle_buffer_follows_the_shorter_side() {
        let floats = |w, h| middle_len(w, h).unwrap();
        assert_eq!(floats(1, 99_999_999), SIDE * 3);
        assert_eq!(floats(99_999_999, 1), SIDE * 3);
        assert_eq!(floats(10_000, 10_000), 10_000 * SIDE * 3);
        assert_eq!(floats(300, 200), 200 * SIDE * 3);
        assert_eq!(floats(200, 300), 200 * SIDE * 3);
    }

    /// A very long, thin picture is prepared, in either direction, and the two directions agree
    /// exactly: one is the other turned a quarter.
    #[test]
    fn a_long_thin_picture_is_prepared_the_same_turned_either_way() {
        let n = 50_000usize;
        let column: Vec<u8> = (0..n)
            .flat_map(|y| [(y % 251) as u8, (y % 13) as u8, 7])
            .collect();
        let tall = preprocess(&column, 1, n).unwrap();
        let wide = preprocess(&column, n, 1).unwrap();
        for c in 0..3 {
            for i in 0..SIDE {
                for j in 0..SIDE {
                    let a = tall[(c * SIDE + i) * SIDE + j];
                    let b = wide[(c * SIDE + j) * SIDE + i];
                    assert!((a - b).abs() < 1e-6, "channel {c} at {i},{j}");
                }
            }
        }
    }

    /// A mask size that overflows is an error, not an overflow or an allocation abort.
    #[test]
    fn a_mask_size_that_overflows_or_cannot_be_held_is_an_error() {
        assert!(coverage(&[], usize::MAX, 2).is_err());
        assert!(coverage(&[0.0; MASK_SIDE * MASK_SIDE], usize::MAX / 2, 2).is_err());
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
        let a = coverage(&logits, 8, 2).unwrap();
        assert_eq!(a.len(), 16);
        assert!(a[0] > 0.99 && a[7] < 0.01);
    }

    /// No logits, or too few, give zero coverage everywhere.
    #[test]
    fn anything_but_a_full_mask_gives_an_empty_one() {
        assert!(coverage(&[], 3, 3).unwrap().iter().all(|v| *v == 0.0));
        assert!(
            coverage(&[5.0; 10], 3, 3)
                .unwrap()
                .iter()
                .all(|v| *v == 0.0)
        );
    }
}
