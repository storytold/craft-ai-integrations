//! The cutout recipe: what is the subject (BEN2), then exactly how much of each edge pixel is
//! subject (ViTMatte). Tuned on 67-megapixel photos against hard cases: pineapple leaves and pine
//! needles on a backdrop of their own colour, hair, a car's roof rails, 3D lettering.
//!
//! 1. **BEN2, whole picture** (its 1024 px square): finds the subject.
//! 2. **BEN2 again along the contour**, in 1024 px tiles of a working picture of 3072 px. The
//!    closer look may sharpen and remove (backdrop caught between leaves) but adds only where the
//!    first look already saw something, so a gap it rejected stays rejected (roof rails).
//! 3. **ViTMatte along the contour**, at up to 6144 px, from a trimap of that matte. With
//!    [`Quality::Max`], tiles whose contour breaks into strands (hair) are matted again with the
//!    larger model.

use craft_ai_core::AiError;

use super::plane::{Plane, Tile, bilinear, dilate, erode, mask, ramp_weight, tiles};

/// The square both models' tiles are.
pub const SIDE: usize = 1024;
/// BEN2's working picture: its closer look tiles a picture of this size.
pub const WORK_SIDE: usize = 3072;
/// ViTMatte's picture: edges are matted at up to this size, then scaled to the picture.
pub const MATTE_SIDE: usize = 6144;

/// How hard the edge is matted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// The small matting model everywhere: best on products, lettering and hard-edged objects.
    High,
    /// Also the larger matting model wherever the edge breaks into strands: for people with hair.
    Max,
}

/// The models the recipe runs. The real one is rten; tests give a fake.
pub trait Models {
    /// BEN2 on a 1 × 3 × 1024 × 1024 picture (0–1, planar): the subject's matte, 1024 × 1024,
    /// as probabilities or logits.
    fn subject(&mut self, input: Vec<f32>) -> Result<Vec<f32>, AiError>;
    /// ViTMatte on a 1 × 4 × `h` × `w` input (normalised RGB and trimap): the alpha, `h` × `w`.
    /// `large` asks for the base model.
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
    /// From 8-bit RGB bytes.
    pub fn from_bytes(w: usize, h: usize, bytes: &[u8]) -> Self {
        let px = bytes
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| p.map(|v| f32::from(v) / 255.0))
            .collect();
        Self { w, h, px }
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

/// The subject's alpha at the size of `picture`.
pub fn cutout(
    picture: &Rgb,
    models: &mut dyn Models,
    quality: Quality,
    ctl: &Ctl,
) -> Result<Plane, AiError> {
    if picture.w < 8 || picture.h < 8 || picture.px.len() != picture.w * picture.h {
        return Err(AiError::invalid("The picture is too small to cut out."));
    }
    let matting = picture.fit(MATTE_SIDE);
    let work = matting.fit(WORK_SIDE);
    let coarse = subject(&work, models, ctl)?.resized(matting.w, matting.h);
    (ctl.progress)(0.45);
    let alpha = matte(&matting, &coarse, models, quality, ctl)?;
    Ok(alpha.resized(picture.w, picture.h))
}

fn sigmoid_if_logits(mut v: Vec<f32>) -> Vec<f32> {
    let logits = v.iter().any(|x| *x < -1e-3 || *x > 1.0 + 1e-3);
    for x in &mut v {
        let y = if logits {
            1.0 / (1.0 + (-*x).exp())
        } else {
            *x
        };
        *x = if y.is_finite() {
            y.clamp(0.0, 1.0)
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
        v: sigmoid_if_logits(out),
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
    let (grown, shrunk) = (dilate(&inside, d), erode(&inside, d));
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
        add_tile(&mut acc, &mut ws, *t, &m, &ramp_weight(*t, ov));
        (ctl.progress)(0.05 + 0.4 * (k + 1) as f32 / todo.len().max(1) as f32);
    }
    Ok(firm(&combine_close_look(&g, &acc, &ws, &band)))
}

fn any_in(m: &Plane, t: Tile) -> bool {
    (t.y..t.y + t.h).any(|y| {
        m.v[y * m.w + t.x..y * m.w + t.x + t.w]
            .iter()
            .any(|v| *v > 0.5)
    })
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

/// Steepens a matte around 50% (`(v − ½) · 2.5 + ½`): BEN2's edge, scaled up, spreads over
/// several pixels; the trimap needs to know where it is.
fn firm(p: &Plane) -> Plane {
    Plane {
        w: p.w,
        h: p.h,
        v: p.v
            .iter()
            .map(|v| ((v - 0.5) * 2.5 + 0.5).clamp(0.0, 1.0))
            .collect(),
    }
}

/// ViTMatte on one tile of `img` with its trimap (0, 0.5, 1), padded to a multiple of 32.
fn vitmatte(
    models: &mut dyn Models,
    large: bool,
    img: &Rgb,
    tri: &Plane,
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
            input[3 * n + d] = tri.v[s];
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

/// How many times the contour of `p` crosses `level` in tile `t`.
fn crossings(p: &Plane, t: Tile, level: f32) -> usize {
    let at = |x: usize, y: usize| p.v[(t.y + y) * p.w + t.x + x] > level;
    let mut n = 0;
    for y in 0..t.h {
        for x in 0..t.w {
            if x + 1 < t.w && at(x, y) != at(x + 1, y) {
                n += 1;
            }
            if y + 1 < t.h && at(x, y) != at(x, y + 1) {
                n += 1;
            }
        }
    }
    n
}

/// Whether the matted contour in `t` breaks into strands compared with the coarse one: hair and
/// fur do (ratio ≥ 1.05 of crossings at ¼, ½ and ¾), lettering and car bodies stay near 1.
fn breaks_into_strands(fine: &Plane, coarse: &Plane, t: Tile) -> bool {
    let c = crossings(coarse, t, 0.5);
    if c < 50 {
        return false;
    }
    let f = (crossings(fine, t, 0.25) + crossings(fine, t, 0.5) + crossings(fine, t, 0.75)) as f32
        / 3.0;
    f / c as f32 >= 1.05
}

/// Step 3: the edge alpha at `img`'s size from the coarse matte.
fn matte(
    img: &Rgb,
    coarse: &Plane,
    models: &mut dyn Models,
    quality: Quality,
    ctl: &Ctl,
) -> Result<Plane, AiError> {
    let (w, h) = (img.w, img.h);
    let unk = ((w.max(h) as f32 * 0.0025).round() as usize).max(8);
    let fg = erode(&mask(w, h, |i| coarse.v[i] > 0.98), unk);
    let bg = erode(&mask(w, h, |i| coarse.v[i] < 0.02), unk);
    let tri = Plane::from_fn(w, h, |i| {
        if fg.v[i] > 0.5 {
            1.0
        } else if bg.v[i] > 0.5 {
            0.0
        } else {
            0.5
        }
    });
    let unknown = mask(w, h, |i| tri.v[i] == 0.5);
    let ov = 96;
    let todo: Vec<Tile> = tiles(w, h, SIDE, ov)
        .into_iter()
        .filter(|t| any_in(&unknown, *t))
        .collect();
    let mut acc = Plane::new(w, h, 0.0);
    let mut ws = Plane::new(w, h, 0.0);
    for (k, t) in todo.iter().enumerate() {
        (ctl.check)()?;
        let m = vitmatte(models, false, img, &tri, *t)?;
        add_tile(&mut acc, &mut ws, *t, &m, &ramp_weight(*t, ov));
        (ctl.progress)(0.45 + 0.4 * (k + 1) as f32 / todo.len().max(1) as f32);
    }
    let first = Plane::from_fn(w, h, |i| {
        if ws.v[i] > 0.0 {
            acc.v[i] / ws.v[i]
        } else {
            coarse.v[i]
        }
    });
    let mut out = first.clone();
    if quality == Quality::Max {
        let hair: Vec<Tile> = todo
            .iter()
            .copied()
            .filter(|t| breaks_into_strands(&first, coarse, *t))
            .collect();
        let mut acc = Plane::new(w, h, 0.0);
        let mut ws = Plane::new(w, h, 0.0);
        for (k, t) in hair.iter().enumerate() {
            (ctl.check)()?;
            let m = vitmatte(models, true, img, &tri, *t)?;
            add_tile(&mut acc, &mut ws, *t, &m, &ramp_weight(*t, ov));
            (ctl.progress)(0.85 + 0.1 * (k + 1) as f32 / hair.len().max(1) as f32);
        }
        for i in 0..w * h {
            if ws.v[i] > 0.0 {
                let b = (ws.v[i] / ov as f32).clamp(0.0, 1.0);
                out.v[i] = first.v[i] * (1.0 - b) + acc.v[i] / ws.v[i] * b;
            }
        }
    }
    for i in 0..w * h {
        if tri.v[i] != 0.5 {
            out.v[i] = tri.v[i];
        }
    }
    (ctl.progress)(0.95);
    Ok(out)
}

#[cfg(test)]
mod tests;
