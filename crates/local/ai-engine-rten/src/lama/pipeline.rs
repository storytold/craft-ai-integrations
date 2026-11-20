//! Context-cropped 512 x 512 inpainting pipeline for LaMa.
//!
//! LaMa's published ONNX graph runs at a fixed 512 x 512 resolution. Rather than squashing a high-
//! resolution photograph down to 512 pixels on a side, [`remove`] crops a context window around
//! the marked bounding box (at least 512 pixels where the picture allows it, or padded by symmetric
//! reflection when the picture is smaller), dilates the mask slightly so halo pixels around the
//! object are replaced too, and blends across that outer dilated rim so every pixel outside the
//! rim stays identical to the source picture.

use craft_ai_core::AiError;

use crate::picture::Rgb;

/// Fixed spatial side of `Carve/LaMa-ONNX` (`lama_fp32.onnx`).
pub const SIDE: usize = 512;

/// Seam between [`remove`] and the ONNX runtime so unit tests run without the 208 MB weights file.
pub(crate) trait Inpainter {
    /// Runs LaMa on `image` (`[3, 512, 512]`, `0..1`) and `mask` (`[1, 512, 512]`, `0` or `1`),
    /// returning `[3, 512, 512]` RGB values in `0..255`.
    fn inpaint(&mut self, image: Vec<f32>, mask: Vec<f32>) -> Result<Vec<f32>, AiError>;
}

/// Cancellation and progress callbacks for one run.
pub(crate) struct Ctl<'a> {
    pub check: &'a dyn Fn() -> Result<(), AiError>,
    pub progress: &'a dyn Fn(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl Rect {
    fn width(self) -> usize {
        self.x1.saturating_sub(self.x0)
    }

    fn height(self) -> usize {
        self.y1.saturating_sub(self.y0)
    }
}

/// Bounding box `[x0..x1, y0..y1]` of non-zero bytes in `mask` (`w * h`).
fn mask_bounds(mask: &[u8], w: usize, h: usize) -> Option<Rect> {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
    for (y, row) in mask.chunks_exact(w).take(h).enumerate() {
        for (x, &v) in row.iter().enumerate() {
            if v > 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x0 < x1 && y0 < y1).then_some(Rect { x0, y0, x1, y1 })
}

/// Context crop around `target` inside a `w x h` picture: at least [`SIDE`] where the picture is
/// large enough, and at least 1.5x the dilated target's longer side so surrounding texture is seen.
fn context_crop(target: Rect, w: usize, h: usize) -> Rect {
    let longest = target.width().max(target.height());
    let want = longest.saturating_mul(3).div_ceil(2).max(SIDE);
    let cw = want.min(w);
    let ch = want.min(h);
    let cx = target.x0 + target.width() / 2;
    let cy = target.y0 + target.height() / 2;
    let x0 = cx.saturating_sub(cw / 2).min(w.saturating_sub(cw));
    let y0 = cy.saturating_sub(ch / 2).min(h.saturating_sub(ch));
    Rect {
        x0,
        y0,
        x1: x0 + cw,
        y1: y0 + ch,
    }
}

/// Symmetric reflection index into `0..len` (matching `np.pad(..., mode="symmetric")`).
fn reflect(idx: usize, len: usize) -> usize {
    if len <= 1 {
        return 0;
    }
    let period = 2 * len;
    let m = idx % period;
    if m < len { m } else { period - 1 - m }
}

/// Computes per-pixel Manhattan distance inside `crop` to the nearest marked pixel (`mask > 0`),
/// capped at `rim + 1`. `0` means the pixel itself is marked; `1..=rim` is the outer feather rim.
fn rim_distance(mask: &[u8], full_w: usize, crop: Rect, rim: usize) -> Vec<u8> {
    let (cw, ch) = (crop.width(), crop.height());
    let cap = u8::try_from(rim.saturating_add(1)).unwrap_or(u8::MAX);
    let mut dist = vec![cap; cw * ch];
    for y in 0..ch {
        let src_row = (crop.y0 + y) * full_w + crop.x0;
        for x in 0..cw {
            if mask.get(src_row + x).copied().unwrap_or(0) > 0
                && let Some(d) = dist.get_mut(y * cw + x)
            {
                *d = 0;
            }
        }
    }
    // Forward pass (top-left to bottom-right).
    for y in 0..ch {
        for x in 0..cw {
            let mut best = dist.get(y * cw + x).copied().unwrap_or(cap);
            if y > 0 {
                best = best.min(dist[(y - 1) * cw + x].saturating_add(1));
            }
            if x > 0 {
                best = best.min(dist[y * cw + (x - 1)].saturating_add(1));
            }
            dist[y * cw + x] = best.min(cap);
        }
    }
    // Backward pass (bottom-right to top-left).
    for y in (0..ch).rev() {
        for x in (0..cw).rev() {
            let mut best = dist[y * cw + x];
            if y + 1 < ch {
                best = best.min(dist[(y + 1) * cw + x].saturating_add(1));
            }
            if x + 1 < cw {
                best = best.min(dist[y * cw + (x + 1)].saturating_add(1));
            }
            dist[y * cw + x] = best.min(cap);
        }
    }
    dist
}

/// Builds `[3, SIDE, SIDE]` (`0..1`) and `[1, SIDE, SIDE]` (`0` or `1`) tensors for `crop`.
fn prepare_inputs(
    rgb: &[u8],
    full_w: usize,
    crop: Rect,
    dist: &[u8],
    rim: u8,
) -> (Vec<f32>, Vec<f32>) {
    let (cw, ch) = (crop.width(), crop.height());
    let plane = SIDE * SIDE;
    let mut image = vec![0.0f32; 3 * plane];
    let mut mask = vec![0.0f32; plane];
    let direct = cw <= SIDE && ch <= SIDE;

    for oy in 0..SIDE {
        for ox in 0..SIDE {
            let (sx, sy, m_val) = if direct {
                let sx = reflect(ox, cw);
                let sy = reflect(oy, ch);
                let d = dist.get(sy * cw + sx).copied().unwrap_or(u8::MAX);
                (sx, sy, if d <= rim { 1.0 } else { 0.0 })
            } else {
                // Downsample image at pixel center; max-pool dilated mask across footprint so thin
                // strokes never vanish when a large crop is scaled down to 512.
                let x0 = ox * cw / SIDE;
                let x1 = ((ox + 1) * cw).div_ceil(SIDE).min(cw).max(x0 + 1);
                let y0 = oy * ch / SIDE;
                let y1 = ((oy + 1) * ch).div_ceil(SIDE).min(ch).max(y0 + 1);
                let mut any_masked = false;
                for my in y0..y1 {
                    for mx in x0..x1 {
                        if dist.get(my * cw + mx).copied().unwrap_or(u8::MAX) <= rim {
                            any_masked = true;
                        }
                    }
                }
                let cx = ((ox * 2 + 1) * cw / (2 * SIDE)).min(cw - 1);
                let cy = ((oy * 2 + 1) * ch / (2 * SIDE)).min(ch - 1);
                (cx, cy, if any_masked { 1.0 } else { 0.0 })
            };
            let px_idx = ((crop.y0 + sy) * full_w + (crop.x0 + sx)) * 3;
            for c in 0..3 {
                let byte = rgb.get(px_idx + c).copied().unwrap_or(0);
                image[c * plane + oy * SIDE + ox] = f32::from(byte) / 255.0;
            }
            mask[oy * SIDE + ox] = m_val;
        }
    }
    (image, mask)
}

/// Samples `[3, SIDE, SIDE]` predicted RGB (`0..255`) at `(x, y)` inside a `cw x ch` crop.
fn sample_prediction(pred: &[f32], cw: usize, ch: usize, x: usize, y: usize) -> [f32; 3] {
    let plane = SIDE * SIDE;
    let sanitize = |v: f32| {
        if v.is_finite() {
            v.clamp(0.0, 255.0)
        } else {
            0.0
        }
    };
    if cw <= SIDE && ch <= SIDE {
        let idx = y * SIDE + x;
        return [
            sanitize(pred.get(idx).copied().unwrap_or(0.0)),
            sanitize(pred.get(plane + idx).copied().unwrap_or(0.0)),
            sanitize(pred.get(2 * plane + idx).copied().unwrap_or(0.0)),
        ];
    }
    let fx = ((x as f32 + 0.5) * (SIDE as f32 / cw as f32) - 0.5).clamp(0.0, (SIDE - 1) as f32);
    let fy = ((y as f32 + 0.5) * (SIDE as f32 / ch as f32) - 0.5).clamp(0.0, (SIDE - 1) as f32);
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(SIDE - 1), (y0 + 1).min(SIDE - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let mut rgb = [0.0f32; 3];
    for (c, slot) in rgb.iter_mut().enumerate() {
        let base = c * plane;
        let p00 = sanitize(pred[base + y0 * SIDE + x0]);
        let p10 = sanitize(pred[base + y0 * SIDE + x1]);
        let p01 = sanitize(pred[base + y1 * SIDE + x0]);
        let p11 = sanitize(pred[base + y1 * SIDE + x1]);
        let top = p00 * (1.0 - tx) + p10 * tx;
        let bot = p01 * (1.0 - tx) + p11 * tx;
        *slot = top * (1.0 - ty) + bot * ty;
    }
    rgb
}

/// Runs context-cropped LaMa removal over `picture` and returns the full-resolution RGB bytes.
pub(crate) fn remove(
    mut picture: Rgb,
    mask: &[u8],
    model: &mut impl Inpainter,
    ctl: &Ctl<'_>,
) -> Result<Vec<u8>, AiError> {
    let (w, h) = (picture.width, picture.height);
    let count = w
        .checked_mul(h)
        .ok_or_else(|| AiError::invalid("The picture is too large."))?;
    if w == 0 || h == 0 || mask.len() != count || picture.bytes.len() != count.saturating_mul(3) {
        return Err(AiError::invalid("The mask and picture sizes do not match."));
    }
    let raw_box = mask_bounds(mask, w, h)
        .ok_or_else(|| AiError::invalid("The mask marks nothing to remove."))?;
    let rim = (raw_box.width().max(raw_box.height()) / 16).clamp(1, 12);
    let dilated_box = Rect {
        x0: raw_box.x0.saturating_sub(rim),
        y0: raw_box.y0.saturating_sub(rim),
        x1: (raw_box.x1 + rim).min(w),
        y1: (raw_box.y1 + rim).min(h),
    };
    let crop = context_crop(dilated_box, w, h);
    let rim_u8 = u8::try_from(rim).unwrap_or(12);
    let dist = rim_distance(mask, w, crop, rim);
    let (img_in, mask_in) = prepare_inputs(&picture.bytes, w, crop, &dist, rim_u8);

    (ctl.check)()?;
    (ctl.progress)(20);
    let predicted = model.inpaint(img_in, mask_in)?;
    let expected_out = 3 * SIDE * SIDE;
    if predicted.len() != expected_out {
        return Err(AiError::invalid(
            "The LaMa model returned an answer of the wrong size.",
        ));
    }
    (ctl.check)()?;
    (ctl.progress)(85);

    let (cw, ch) = (crop.width(), crop.height());
    let feather_denom = (rim as f32 + 1.0).max(1.0);
    for y in 0..ch {
        for x in 0..cw {
            let d = dist[y * cw + x];
            if d > rim_u8 {
                continue;
            }
            // Original marked pixels (`d == 0`) get weight 1.0 so no object remnant bleeds through;
            // the outer dilated rim (`1..=rim`) tapers linearly to 0.0 at `rim + 1`.
            let alpha = if d == 0 {
                1.0
            } else {
                1.0 - (f32::from(d) / feather_denom)
            };
            let pred_rgb = sample_prediction(&predicted, cw, ch, x, y);
            let dst = ((crop.y0 + y) * w + (crop.x0 + x)) * 3;
            for (c, &pred_ch) in pred_rgb.iter().enumerate() {
                if let Some(slot) = picture.bytes.get_mut(dst + c) {
                    let orig = f32::from(*slot);
                    *slot = (orig * (1.0 - alpha) + pred_ch * alpha)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
    (ctl.progress)(95);
    Ok(picture.bytes)
}
