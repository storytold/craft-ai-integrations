//! Context-cropped 512 x 512 inpainting pipeline for LaMa.
//!
//! LaMa's published ONNX graph runs at a fixed 512 x 512 resolution. Rather than squashing a high-
//! resolution photograph down to 512 pixels on a side, [`remove`] works on one marked area at a
//! time (`regions` tells marks that are far apart from each other, so two small marks in opposite
//! corners are not filled from one picture-wide window). For each area it crops a context window
//! around the marked bounding box (at least 512 pixels where the picture allows it), dilates the
//! mask slightly so halo pixels around the object are replaced too, and blends across that outer
//! dilated rim so every pixel outside the rim stays identical to the source picture.
//!
//! A window larger than 512 pixels is scaled down by one factor on both sides, so its shape is
//! kept, with every model pixel the average of the picture pixels it covers. Whatever the scaled
//! window leaves of the 512 x 512 square, on a small picture or a window that is not square, is
//! padded by symmetric reflection.

use craft_ai_core::AiError;

use crate::picture::Rgb;

mod regions;

/// Fixed spatial side of `Carve/LaMa-ONNX` (`lama_fp32.onnx`).
pub const SIDE: usize = 512;

/// The widest feather rim around a marked area, in picture pixels.
const MAX_RIM: usize = 12;

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

    fn longest(self) -> usize {
        self.width().max(self.height())
    }

    /// The smallest box holding both.
    fn union(self, other: Rect) -> Rect {
        Rect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    /// This box widened by `by` pixels on every side, kept inside a `w x h` picture.
    fn grown(self, by: usize, w: usize, h: usize) -> Rect {
        Rect {
            x0: self.x0.saturating_sub(by),
            y0: self.y0.saturating_sub(by),
            x1: self.x1.saturating_add(by).min(w),
            y1: self.y1.saturating_add(by).min(h),
        }
    }

    /// Whether the two boxes share a pixel.
    fn touches(self, other: Rect) -> bool {
        self.x0 < other.x1 && other.x0 < self.x1 && self.y0 < other.y1 && other.y0 < self.y1
    }
}

/// One area's window into the picture, and how the window lies on the model's square.
struct Window {
    /// The part of the picture the model sees.
    crop: Rect,
    /// The crop's width once scaled to fit the square: the crop's own width when it already fits.
    sw: usize,
    /// The crop's height once scaled to fit the square.
    sh: usize,
    /// The feather rim around the marked pixels, in picture pixels.
    rim: u8,
}

/// Context crop around `target` inside a `w x h` picture: at least [`SIDE`] where the picture is
/// large enough, and at least 1.5x the dilated target's longer side so surrounding texture is seen.
fn context_crop(target: Rect, w: usize, h: usize) -> Rect {
    let want = target.longest().saturating_mul(3).div_ceil(2).max(SIDE);
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

/// The size a `cw x ch` crop has on the model's square: unchanged when it fits, otherwise both
/// sides scaled by the one factor that brings the longer side to [`SIDE`], so the shape is kept.
fn scaled_size(cw: usize, ch: usize) -> (usize, usize) {
    let longest = cw.max(ch);
    if longest <= SIDE {
        return (cw, ch);
    }
    let scaled = |n: usize| (n * SIDE).div_ceil(longest).clamp(1, SIDE);
    (scaled(cw), scaled(ch))
}

/// The crop pixels `start..end` that pixel `o` of the scaled side covers, on a side of `len`
/// crop pixels scaled to `scaled`. Exactly pixel `o` when the side is not scaled.
fn footprint(o: usize, len: usize, scaled: usize) -> (usize, usize) {
    let scaled = scaled.max(1);
    let start = o * len / scaled;
    let end = ((o + 1) * len).div_ceil(scaled).min(len).max(start + 1);
    (start, end)
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

/// Computes per-pixel Manhattan distance inside `crop` to the nearest marked pixel (`mask > 0`)
/// of the area `own`, capped at `rim + 1`. `0` means the pixel itself is marked; `1..=rim` is the
/// outer feather rim. Marks of other areas are left to their own run.
fn rim_distance(mask: &[u8], full_w: usize, crop: Rect, own: Rect, rim: usize) -> Vec<u8> {
    let (cw, ch) = (crop.width(), crop.height());
    let cap = u8::try_from(rim.saturating_add(1)).unwrap_or(u8::MAX);
    let mut dist = vec![cap; cw * ch];
    for y in own.y0.max(crop.y0)..own.y1.min(crop.y1) {
        let src_row = y * full_w;
        let dst_row = (y - crop.y0) * cw;
        for x in own.x0.max(crop.x0)..own.x1.min(crop.x1) {
            if mask.get(src_row + x).copied().unwrap_or(0) > 0
                && let Some(d) = dist.get_mut(dst_row + (x - crop.x0))
            {
                *d = 0;
            }
        }
    }
    spread(&mut dist, cw, ch, cap);
    dist
}

/// Turns seeds (`0`) in a `cw x ch` grid into Manhattan distances capped at `cap`, in two passes.
fn spread(dist: &mut [u8], cw: usize, ch: usize, cap: u8) {
    // Forward pass (top-left to bottom-right).
    for y in 0..ch {
        for x in 0..cw {
            let mut best = dist[y * cw + x];
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
}

/// The average colour (`0..1`) of the crop pixels in `cell`, and whether any of them is to be
/// filled: within the rim of this area, or marked at all, so the model never copies from an
/// object another run removes.
fn average(
    rgb: &[u8],
    mask: &[u8],
    full_w: usize,
    win: &Window,
    dist: &[u8],
    cell: Rect,
) -> ([f32; 3], bool) {
    let cw = win.crop.width();
    let mut sum = [0u32; 3];
    let mut count = 0u32;
    let mut marked = false;
    for y in cell.y0..cell.y1 {
        let row = (win.crop.y0 + y) * full_w + win.crop.x0;
        for x in cell.x0..cell.x1 {
            let near = dist.get(y * cw + x).copied().unwrap_or(u8::MAX) <= win.rim;
            marked |= near || mask.get(row + x).copied().unwrap_or(0) > 0;
            for (c, total) in sum.iter_mut().enumerate() {
                *total += u32::from(rgb.get((row + x) * 3 + c).copied().unwrap_or(0));
            }
            count += 1;
        }
    }
    let scale = count.max(1) as f32 * 255.0;
    (sum.map(|total| total as f32 / scale), marked)
}

/// Fills what the scaled `sw x sh` window leaves of the square by symmetric reflection.
fn pad_by_reflection(image: &mut [f32], hole: &mut [f32], sw: usize, sh: usize) {
    if sw >= SIDE && sh >= SIDE {
        return;
    }
    let plane = SIDE * SIDE;
    for oy in 0..SIDE {
        for ox in 0..SIDE {
            if ox < sw && oy < sh {
                continue;
            }
            let from = reflect(oy, sh) * SIDE + reflect(ox, sw);
            let to = oy * SIDE + ox;
            hole[to] = hole[from];
            for c in 0..3 {
                image[c * plane + to] = image[c * plane + from];
            }
        }
    }
}

/// Builds `[3, SIDE, SIDE]` (`0..1`) and `[1, SIDE, SIDE]` (`0` or `1`) tensors for `win`.
///
/// Each model pixel is the average of the crop pixels it covers (one pixel when the crop is not
/// scaled), and is marked when any of them is, so thin strokes never vanish when a large crop is
/// scaled down.
fn prepare_inputs(
    rgb: &[u8],
    mask: &[u8],
    full_w: usize,
    win: &Window,
    dist: &[u8],
) -> (Vec<f32>, Vec<f32>) {
    let plane = SIDE * SIDE;
    let mut image = vec![0.0f32; 3 * plane];
    let mut hole = vec![0.0f32; plane];
    for oy in 0..win.sh.min(SIDE) {
        let (y0, y1) = footprint(oy, win.crop.height(), win.sh);
        for ox in 0..win.sw.min(SIDE) {
            let (x0, x1) = footprint(ox, win.crop.width(), win.sw);
            let cell = Rect { x0, y0, x1, y1 };
            let (colour, marked) = average(rgb, mask, full_w, win, dist, cell);
            let at = oy * SIDE + ox;
            for (c, value) in colour.iter().enumerate() {
                image[c * plane + at] = *value;
            }
            if marked {
                hole[at] = 1.0;
            }
        }
    }
    pad_by_reflection(&mut image, &mut hole, win.sw, win.sh);
    (image, hole)
}

/// A model value as a colour sample: anything that is not a number becomes black.
fn sanitize(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 255.0)
    } else {
        0.0
    }
}

/// Where crop pixel `p` (of `len`) lies on a side scaled to `scaled`: the two scaled pixels
/// either side of it and the weight of the second. Never reaches into the reflected padding.
fn lerp_at(p: usize, len: usize, scaled: usize) -> (usize, usize, f32) {
    let last = scaled.saturating_sub(1);
    let f = ((p as f32 + 0.5) * (scaled as f32 / len.max(1) as f32) - 0.5).clamp(0.0, last as f32);
    let lo = (f.floor() as usize).min(last);
    (lo, (lo + 1).min(last), f - lo as f32)
}

/// Samples `[3, SIDE, SIDE]` predicted RGB (`0..255`) at `(x, y)` inside the crop of `win`.
fn sample_prediction(pred: &[f32], win: &Window, x: usize, y: usize) -> [f32; 3] {
    let plane = SIDE * SIDE;
    let (cw, ch) = (win.crop.width(), win.crop.height());
    let at = |c: usize, px: usize, py: usize| {
        sanitize(pred.get(c * plane + py * SIDE + px).copied().unwrap_or(0.0))
    };
    if win.sw == cw && win.sh == ch {
        return [at(0, x, y), at(1, x, y), at(2, x, y)];
    }
    let (x0, x1, tx) = lerp_at(x, cw, win.sw);
    let (y0, y1, ty) = lerp_at(y, ch, win.sh);
    let mut rgb = [0.0f32; 3];
    for (c, slot) in rgb.iter_mut().enumerate() {
        let top = at(c, x0, y0) * (1.0 - tx) + at(c, x1, y0) * tx;
        let bot = at(c, x0, y1) * (1.0 - tx) + at(c, x1, y1) * tx;
        *slot = top * (1.0 - ty) + bot * ty;
    }
    rgb
}

/// Writes the model's answer into the picture over the marked pixels of one area and its rim.
fn blend(picture: &mut Rgb, win: &Window, dist: &[u8], predicted: &[f32]) {
    let (cw, ch) = (win.crop.width(), win.crop.height());
    let w = picture.width;
    let rim_u8 = win.rim;
    let feather_denom = f32::from(rim_u8) + 1.0;
    for y in 0..ch {
        for x in 0..cw {
            let d = dist.get(y * cw + x).copied().unwrap_or(u8::MAX);
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
            let pred_rgb = sample_prediction(predicted, win, x, y);
            let dst = ((win.crop.y0 + y) * w + (win.crop.x0 + x)) * 3;
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
}

/// Fills one marked `area` of the picture from its own context window.
fn fill_area(
    picture: &mut Rgb,
    mask: &[u8],
    area: Rect,
    model: &mut impl Inpainter,
) -> Result<(), AiError> {
    let (w, h) = (picture.width, picture.height);
    let rim = (area.longest() / 16).clamp(1, MAX_RIM);
    let crop = context_crop(area.grown(rim, w, h), w, h);
    let (sw, sh) = scaled_size(crop.width(), crop.height());
    let win = Window {
        crop,
        sw,
        sh,
        rim: u8::try_from(rim).unwrap_or(u8::MAX),
    };
    let dist = rim_distance(mask, w, crop, area, rim);
    let (img_in, mask_in) = prepare_inputs(&picture.bytes, mask, w, &win, &dist);
    let predicted = model.inpaint(img_in, mask_in)?;
    let expected_out = 3 * SIDE * SIDE;
    if predicted.len() != expected_out {
        return Err(AiError::invalid(
            "The LaMa model returned an answer of the wrong size.",
        ));
    }
    blend(picture, &win, &dist, &predicted);
    Ok(())
}

/// Progress, 20 to 85, once `done` of `total` areas are filled.
fn progress_at(done: usize, total: usize) -> u8 {
    u8::try_from(20 + 65 * done / total.max(1)).unwrap_or(85)
}

/// Runs context-cropped LaMa removal over `picture` and returns the full-resolution RGB bytes.
///
/// The model runs once for each separate marked area (see `regions`), and Cancel is checked
/// between runs.
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
    let areas = regions::areas(mask, w, h);
    if areas.is_empty() {
        return Err(AiError::invalid("The mask marks nothing to remove."));
    }
    for (done, area) in areas.iter().enumerate() {
        (ctl.check)()?;
        (ctl.progress)(progress_at(done, areas.len()));
        fill_area(&mut picture, mask, *area, model)?;
    }
    (ctl.check)()?;
    (ctl.progress)(95);
    Ok(picture.bytes)
}

#[cfg(test)]
mod tests;
