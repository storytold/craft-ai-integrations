//! The cutout recipe: what is the subject (BEN2), then exactly how much of each edge pixel is
//! subject (ViTMatte). Tuned on 67-megapixel photos against hard cases (pineapple leaves and pine
//! needles on a backdrop of their own colour, hair, a car's roof rails, 3D lettering), then scored
//! on AIM-500, a public set of photos with hand-made mattes.
//!
//! 1. **BEN2, whole picture** (its 1024 px square): finds the subject.
//! 2. **BEN2 again along the contour**, in 1024 px tiles of a working picture of 3072 px. The
//!    closer look may sharpen and remove (backdrop caught between leaves) but adds only where the
//!    first look already saw something, so a gap it rejected stays rejected (roof rails).
//! 3. **ViTMatte along the contour**, at up to 6144 px, from a trimap of that matte. Each tile is
//!    matted by both the small and the base ViTMatte and the two alphas are averaged: on AIM-500
//!    the pair beat either model alone, hair and hard edges alike.

use craft_ai_core::AiError;

use super::plane::{Plane, Tile, bilinear, dilate, erode, mask, ramp_weight, tiles};

/// The square both models' tiles are.
pub const SIDE: usize = 1024;
/// BEN2's working picture: its closer look tiles a picture of this size.
pub const WORK_SIDE: usize = 3072;
/// ViTMatte's picture: edges are matted at up to this size, then scaled to the picture.
pub const MATTE_SIDE: usize = 6144;

/// How far BEN2's matte is steepened around 50% before the trimap is cut from it.
const FIRM: f32 = 1.5;

/// The trimap's three values, one byte a pixel.
const BACKDROP: u8 = 0;
const UNKNOWN: u8 = 1;
const SUBJECT: u8 = 2;

/// The models the recipe runs. The real one is rten; tests give a fake.
pub trait Models {
    /// BEN2 on a 1 × 3 × 1024 × 1024 picture (0–1, planar): the subject's matte, 1024 × 1024, as
    /// probabilities (0–1). The pinned `BEN2_Base.onnx` ends in a sigmoid.
    fn subject(&mut self, input: Vec<f32>) -> Result<Vec<f32>, AiError>;
    /// ViTMatte on a 1 × 4 × `h` × `w` input (normalised RGB and trimap): the alpha, `h` × `w`.
    /// `large` asks for the base model, otherwise the small one.
    fn matte(
        &mut self,
        large: bool,
        input: Vec<f32>,
        h: usize,
        w: usize,
    ) -> Result<Vec<f32>, AiError>;
}

/// Cancellation and progress (0–1), asked between model runs.
pub struct Ctl<'a> {
    pub check: &'a dyn Fn() -> Result<(), AiError>,
    pub progress: &'a dyn Fn(f32),
}

/// An RGB picture (0–1, as shown), row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Rgb {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 3]>,
}

impl Rgb {
    /// 8-bit RGB `bytes` shrunk to fit `side` on the longer side, as [`Rgb::fit`] would, but
    /// without a full-size float copy: one channel at a time. The bytes are dropped here.
    pub fn fit_bytes(w: usize, h: usize, bytes: Vec<u8>, side: usize) -> Self {
        let long = w.max(h);
        if long <= side || side == 0 {
            let px = bytes
                .as_chunks::<3>()
                .0
                .iter()
                .map(|p| p.map(|v| f32::from(v) / 255.0))
                .collect();
            return Self { w, h, px };
        }
        let (nw, nh) = (
            (w * side).div_ceil(long).max(1),
            (h * side).div_ceil(long).max(1),
        );
        let r = long / side / 2;
        let mut px = vec![[0.0; 3]; nw * nh];
        for c in 0..3 {
            let mut plane = Plane::from_fn(w, h, |i| f32::from(bytes[i * 3 + c]) / 255.0);
            plane.box_blur_in_place(r);
            let small = bilinear(&plane.v, w, h, nw, nh);
            drop(plane);
            for (p, v) in px.iter_mut().zip(small) {
                p[c] = v;
            }
        }
        Self { w: nw, h: nh, px }
    }

    /// This picture shrunk to fit `side` on its longer side (box-filtered, then bilinear), or
    /// itself when it already fits.
    pub fn fit(&self, side: usize) -> Self {
        let long = self.w.max(self.h);
        if long <= side || side == 0 {
            return self.clone();
        }
        let (w, h) = (
            (self.w * side).div_ceil(long).max(1),
            (self.h * side).div_ceil(long).max(1),
        );
        let r = long / side / 2;
        let ch = |c: usize| {
            Plane {
                w: self.w,
                h: self.h,
                v: self.px.iter().map(|p| p[c]).collect(),
            }
            .box_blur(r)
            .resized(w, h)
        };
        let (a, b, c) = (ch(0), ch(1), ch(2));
        Self {
            w,
            h,
            px: (0..w * h).map(|i| [a.v[i], b.v[i], c.v[i]]).collect(),
        }
    }

    fn crop(&self, t: Tile) -> Self {
        let mut px = Vec::with_capacity(t.w * t.h);
        for y in t.y..t.y + t.h {
            px.extend_from_slice(&self.px[y * self.w + t.x..y * self.w + t.x + t.w]);
        }
        Self { w: t.w, h: t.h, px }
    }
}

/// The subject's alpha for a `w` × `h` picture given as 8-bit RGB `bytes`, at that size. The
/// bytes are dropped as soon as the matting picture is made from them.
pub fn cutout(
    w: usize,
    h: usize,
    bytes: Vec<u8>,
    models: &mut dyn Models,
    ctl: &Ctl,
) -> Result<Plane, AiError> {
    if w < 8 || h < 8 || Some(bytes.len()) != w.checked_mul(h).and_then(|n| n.checked_mul(3)) {
        return Err(AiError::invalid("The picture is too small to cut out."));
    }
    let matting = Rgb::fit_bytes(w, h, bytes, MATTE_SIDE);
    let coarse = {
        let work = matting.fit(WORK_SIDE);
        subject(&work, models, ctl)?.resized(matting.w, matting.h)
    };
    (ctl.progress)(0.45);
    let alpha = matte(&matting, coarse, models, ctl)?;
    drop(matting);
    Ok(alpha.resized(w, h))
}

/// BEN2's answer as a matte: its probabilities, clamped, with anything not finite as backdrop.
fn probabilities(mut v: Vec<f32>) -> Vec<f32> {
    for x in &mut v {
        *x = if x.is_finite() {
            x.clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
    v
}

/// BEN2 on `img` scaled to its square input, its matte scaled back to `img`'s size.
fn ben2(models: &mut dyn Models, img: &Rgb) -> Result<Plane, AiError> {
    let sq = bilinear(&img.px, img.w, img.h, SIDE, SIDE);
    let n = SIDE * SIDE;
    let mut input = vec![0.0; 3 * n];
    for (i, p) in sq.iter().enumerate() {
        for c in 0..3 {
            input[c * n + i] = p[c].clamp(0.0, 1.0);
        }
    }
    let out = models.subject(input)?;
    if out.len() != n {
        return Err(AiError::invalid(
            "The subject model gave a matte of the wrong size.",
        ));
    }
    Ok(Plane {
        w: SIDE,
        h: SIDE,
        v: probabilities(out),
    }
    .resized(img.w, img.h))
}

/// Steps 1 and 2: the subject's coarse matte at `img`'s size.
fn subject(img: &Rgb, models: &mut dyn Models, ctl: &Ctl) -> Result<Plane, AiError> {
    (ctl.check)()?;
    let g = ben2(models, img)?;
    (ctl.progress)(0.05);
    // A picture BEN2 already sees at about its own size gains nothing from tiles.
    if img.w.max(img.h) <= SIDE * 5 / 4 {
        return Ok(firm(&g));
    }
    let (w, h) = (img.w, img.h);
    let d = ((w.max(h) as f32 * 0.012).round() as usize).max(6);
    let inside = mask(w, h, |i| g.v[i] > 0.5);
    let (grown, shrunk) = (dilate(inside.clone(), d), erode(inside, d));
    let band = mask(w, h, |i| grown.v[i] > 0.5 && shrunk.v[i] < 0.5);
    let ov = 192;
    let todo: Vec<Tile> = tiles(w, h, SIDE, ov)
        .into_iter()
        .filter(|t| any_in(&band, *t))
        .collect();
    let mut acc = Plane::new(w, h, 0.0);
    let mut ws = Plane::new(w, h, 0.0);
    for (k, t) in todo.iter().enumerate() {
        (ctl.check)()?;
        let m = ben2(models, &img.crop(*t))?;
        add_tile(&mut acc, &mut ws, *t, &m, &ramp_weight(*t, ov, w, h));
        (ctl.progress)(0.05 + 0.4 * (k + 1) as f32 / todo.len().max(1) as f32);
    }
    Ok(firm(&combine_close_look(&g, &acc, &ws, &band)))
}

fn any_in(m: &Plane, t: Tile) -> bool {
    any_where(&m.v, m.w, t, |v| *v > 0.5)
}

fn any_where<T>(v: &[T], w: usize, t: Tile, f: impl Fn(&T) -> bool) -> bool {
    (t.y..t.y + t.h).any(|y| v[y * w + t.x..y * w + t.x + t.w].iter().any(&f))
}

fn add_tile(acc: &mut Plane, ws: &mut Plane, t: Tile, m: &Plane, wt: &Plane) {
    for y in 0..t.h {
        for x in 0..t.w {
            let (i, j) = ((t.y + y) * acc.w + t.x + x, y * t.w + x);
            acc.v[i] += m.v[j] * wt.v[j];
            ws.v[i] += wt.v[j];
        }
    }
}

/// The close look's matte merged into the whole-picture one `g`: freely where it removes, and
/// adds only as far as `g` was above 0.35 (fully from 0.65), with smooth weights so the
/// scaled-up edge has no steps; only inside the contour band, feathered.
fn combine_close_look(g: &Plane, acc: &Plane, ws: &Plane, band: &Plane) -> Plane {
    let allow = Plane::from_fn(g.w, g.h, |i| ((g.v[i] - 0.35) / 0.3).clamp(0.0, 1.0))
        .box_blur(2)
        .box_blur(2);
    let near = Plane::from_fn(g.w, g.h, |i| if ws.v[i] > 0.0 { band.v[i] } else { 0.0 })
        .box_blur(3)
        .box_blur(3);
    Plane::from_fn(g.w, g.h, |i| {
        let gi = g.v[i];
        let tm = if ws.v[i] > 0.0 {
            acc.v[i] / ws.v[i]
        } else {
            gi
        };
        let w = if tm <= gi { 1.0 } else { allow.v[i] };
        let t2 = gi + (tm - gi) * w;
        let b = if ws.v[i] > 0.0 { near.v[i] } else { 0.0 };
        gi * (1.0 - b) + t2 * b
    })
}

/// Steepens a matte around 50% (`(v − ½) · 1.5 + ½`): BEN2's edge, scaled up, spreads over
/// several pixels; the trimap needs to know where it is. Steeper (2.5 was tried) marks too much
/// of a soft edge as certain, and ViTMatte never sees it.
fn firm(p: &Plane) -> Plane {
    Plane {
        w: p.w,
        h: p.h,
        v: p.v
            .iter()
            .map(|v| ((v - 0.5) * FIRM + 0.5).clamp(0.0, 1.0))
            .collect(),
    }
}

/// ViTMatte on one tile of `img` with its trimap, padded to a multiple of 32.
fn vitmatte(
    models: &mut dyn Models,
    large: bool,
    img: &Rgb,
    tri: &[u8],
    t: Tile,
) -> Result<Plane, AiError> {
    let (pw, ph) = (t.w.div_ceil(32) * 32, t.h.div_ceil(32) * 32);
    let n = pw * ph;
    let mut input = vec![0.0; 4 * n];
    for y in 0..t.h {
        for x in 0..t.w {
            let (s, d) = ((t.y + y) * img.w + t.x + x, y * pw + x);
            let p = img.px[s];
            for c in 0..3 {
                input[c * n + d] = (p[c].clamp(0.0, 1.0) - 0.5) / 0.5;
            }
            input[3 * n + d] = match tri[s] {
                SUBJECT => 1.0,
                BACKDROP => 0.0,
                _ => 0.5,
            };
        }
    }
    let out = models.matte(large, input, ph, pw)?;
    if out.len() != n {
        return Err(AiError::invalid(
            "The matting model gave an alpha of the wrong size.",
        ));
    }
    Ok(Plane::from_fn(t.w, t.h, |i| {
        out[(i / t.w) * pw + i % t.w].clamp(0.0, 1.0)
    }))
}

/// The trimap of `coarse`: subject and backdrop where it is sure (above 98%, below 2%) and stays
/// sure within `unk` pixels, unknown elsewhere.
fn trimap(coarse: &Plane, unk: usize) -> Vec<u8> {
    let (w, h) = (coarse.w, coarse.h);
    let mut tri = vec![UNKNOWN; w * h];
    let sure = erode(mask(w, h, |i| coarse.v[i] > 0.98), unk);
    for (t, s) in tri.iter_mut().zip(&sure.v) {
        if *s > 0.5 {
            *t = SUBJECT;
        }
    }
    let sure = erode(mask(w, h, |i| coarse.v[i] < 0.02), unk);
    for (t, s) in tri.iter_mut().zip(&sure.v) {
        if *s > 0.5 {
            *t = BACKDROP;
        }
    }
    tri
}

/// Step 3: the edge alpha at `img`'s size from the coarse matte, which it consumes.
fn matte(img: &Rgb, coarse: Plane, models: &mut dyn Models, ctl: &Ctl) -> Result<Plane, AiError> {
    let (w, h) = (img.w, img.h);
    let unk = ((w.max(h) as f32 * 0.0025).round() as usize).max(8);
    let tri = trimap(&coarse, unk);
    let ov = 96;
    let todo: Vec<Tile> = tiles(w, h, SIDE, ov)
        .into_iter()
        .filter(|t| any_where(&tri, w, *t, |v| *v == UNKNOWN))
        .collect();
    let mut acc = Plane::new(w, h, 0.0);
    let mut ws = Plane::new(w, h, 0.0);
    for (k, t) in todo.iter().enumerate() {
        (ctl.check)()?;
        let mut m = vitmatte(models, false, img, &tri, *t)?;
        (ctl.check)()?;
        let large = vitmatte(models, true, img, &tri, *t)?;
        for (a, b) in m.v.iter_mut().zip(&large.v) {
            *a = (*a + b) / 2.0;
        }
        add_tile(&mut acc, &mut ws, *t, &m, &ramp_weight(*t, ov, w, h));
        (ctl.progress)(0.45 + 0.5 * (k + 1) as f32 / todo.len().max(1) as f32);
    }
    for (((a, t), wt), c) in acc.v.iter_mut().zip(&tri).zip(&ws.v).zip(&coarse.v) {
        *a = match *t {
            SUBJECT => 1.0,
            BACKDROP => 0.0,
            _ if *wt > 0.0 => *a / wt,
            _ => *c,
        };
    }
    (ctl.progress)(0.95);
    Ok(acc)
}

#[cfg(test)]
mod tests;
