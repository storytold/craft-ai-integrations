//! A matting network trained near one megapixel, made usable on a picture of any size (the
//! approach of the Gyre background remover):
//!
//! 1. Up to [`MAX_RES`] pixels on the long side the picture runs at full resolution,
//!    reflect-padded to a multiple of [`ALIGN`] (centred).
//! 2. Larger pictures are reflect-padded to a square (at the left and top) and reduced with
//!    antialiased Lanczos-3 so the square is [`MAX_RES`]²; the matte is enlarged back with
//!    Lanczos-3 and cropped. The square is never built: source rows stream through in strips, and
//!    the enlargement runs row by row.
//! 3. When that scale is below [`GUIDED_BELOW`], the matte is refined with a colour guided filter
//!    (K. He, J. Sun and X. Tang, "Guided Image Filtering", PAMI 2013) guided by the
//!    full-resolution picture, keeping `max(guided, enlarged)`: fine detail (hair, fur) the small
//!    input lost comes back without eroding the body of the matte.
//! 4. A shrink, `(m − s) / (1 − s)`, tightens the soft fringe.

mod guided;
mod resize;

use craft_ai_core::AiError;
use craft_ai_engine::RunContext;

pub use resize::Input;

/// Longest side the network sees (pixels).
pub const MAX_RES: usize = 1280;
/// Input sides are padded to a multiple of this (the network's coarsest stride).
pub const ALIGN: usize = 32;
/// Below this working scale the matte is refined at full resolution by the guided filter.
const GUIDED_BELOW: f64 = 0.5;
/// Guided-filter window radius (full-resolution pixels).
const GUIDED_RADIUS: usize = 32;
/// Guided-filter regulariser. Gyre used 1e-8 with float64 statistics; the box means here are
/// `f32`, so a slightly larger value keeps flat colour areas well-conditioned while still
/// following edges a fraction of an 8-bit level apart.
const GUIDED_EPS: f32 = 1e-6;
/// The fringe shrink (Gyre's default).
pub const SHRINK: f32 = 0.2;

/// An 8-bit RGB picture, row by row, as `craft-ai-engine-rten` decodes it.
#[derive(Clone, Copy)]
pub struct Picture<'a> {
    pub width: usize,
    pub height: usize,
    pub bytes: &'a [u8],
}

impl Picture<'_> {
    /// Rows `y0..y1`, columns `x0..x1` as RGB in `0..=1`. Outside the picture reads black.
    fn rgb(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> Vec<[f32; 3]> {
        let mut out = Vec::with_capacity(x1.saturating_sub(x0) * y1.saturating_sub(y0));
        for y in y0..y1 {
            for x in x0..x1 {
                let at = (y * self.width + x) * 3;
                out.push(match self.bytes.get(at..at + 3) {
                    Some(p) => [
                        p[0] as f32 / 255.0,
                        p[1] as f32 / 255.0,
                        p[2] as f32 / 255.0,
                    ],
                    None => [0.0; 3],
                });
            }
        }
        out
    }
}

/// How a picture maps to the network input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    /// Network pixels per picture pixel (`1.0` = full resolution).
    pub scale: f64,
    /// The padded network input.
    pub in_w: usize,
    pub in_h: usize,
    /// Padding before the picture (left, top), in picture pixels.
    pub left: usize,
    pub top: usize,
}

impl Plan {
    /// Whether the guided filter refines the enlarged matte.
    fn guided(&self) -> bool {
        self.scale < GUIDED_BELOW
    }
}

/// The input layout for a `w × h` picture (`None` if it is empty). `max_res` is rounded down to a
/// multiple of [`ALIGN`], and at least [`ALIGN`].
pub fn plan(w: usize, h: usize, max_res: usize) -> Option<Plan> {
    if w == 0 || h == 0 {
        return None;
    }
    let max_res = (max_res.max(ALIGN) / ALIGN) * ALIGN;
    let long = w.max(h);
    if long > max_res {
        Some(Plan {
            scale: max_res as f64 / long as f64,
            in_w: max_res,
            in_h: max_res,
            left: long - w,
            top: long - h,
        })
    } else {
        let (in_w, in_h) = (w.div_ceil(ALIGN) * ALIGN, h.div_ceil(ALIGN) * ALIGN);
        Some(Plan {
            scale: 1.0,
            in_w,
            in_h,
            left: (in_w - w) / 2,
            top: (in_h - h) / 2,
        })
    }
}

/// 256-entry table of the fringe shrink `(m − s) / (1 − s)`.
fn shrink_lut(shrink: f32) -> [u8; 256] {
    let s = if shrink.is_finite() {
        shrink.clamp(0.0, 0.99)
    } else {
        0.0
    };
    std::array::from_fn(|v| {
        ((((v as f32 / 255.0) - s) / (1.0 - s)).clamp(0.0, 1.0) * 255.0).round() as u8
    })
}

/// The subject's coverage of `picture`, one 8-bit value per pixel, row by row.
///
/// `predict` runs the network on a padded input whose sides are multiples of [`ALIGN`] and
/// returns one foreground probability per input pixel. Progress is reported from 10 to 90;
/// Cancel is checked between stages.
pub fn matte(
    picture: Picture<'_>,
    max_res: usize,
    shrink: f32,
    predict: &mut dyn FnMut(&Input) -> Result<Vec<f32>, AiError>,
    ctx: &RunContext,
) -> Result<Vec<u8>, AiError> {
    let (w, h) = (picture.width, picture.height);
    let n = w
        .checked_mul(h)
        .filter(|n| picture.bytes.len() == n * 3)
        .ok_or_else(|| AiError::invalid("The picture could not be read. Use a PNG or a JPEG."))?;
    let Some(p) = plan(w, h, max_res) else {
        return Ok(Vec::new());
    };

    ctx.check_cancelled()?;
    let input = if p.scale >= 1.0 {
        resize::pad_input(picture, &p)
    } else {
        resize::reduce_input(picture, &p, ctx)?
    };
    ctx.progress(10);

    let mut pred = predict(&input)?;
    if pred.len() != p.in_w * p.in_h {
        return Err(AiError::new(
            craft_ai_core::ErrorKind::Internal,
            "The background remover returned a matte of the wrong size.",
        ));
    }
    // NaN is background; out-of-range values (infinities included) clamp.
    for v in &mut pred {
        *v = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
    }
    ctx.check_cancelled()?;
    ctx.progress(75);

    let mut mask = vec![0u8; n];
    if p.scale >= 1.0 {
        for (y, row) in mask.chunks_exact_mut(w).enumerate() {
            let at = (y + p.top) * p.in_w + p.left;
            for (o, v) in row.iter_mut().zip(pred.get(at..at + w).unwrap_or(&[])) {
                *o = (v * 255.0).round() as u8;
            }
        }
    } else {
        resize::expand_matte(&pred, &p, w, &mut mask);
    }
    ctx.check_cancelled()?;

    if p.guided() {
        guided::refine(picture, &mut mask, GUIDED_RADIUS, GUIDED_EPS, ctx)?;
    }
    ctx.progress(90);

    if shrink > 0.0 {
        let lut = shrink_lut(shrink);
        for m in &mut mask {
            *m = lut[*m as usize];
        }
    }
    Ok(mask)
}

#[cfg(test)]
mod tests;
