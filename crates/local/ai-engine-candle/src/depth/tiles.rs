//! High-quality depth: the whole picture is estimated once (the global pass), then again in
//! overlapping tiles that each get the network's full resolution. Every tile is fitted to the
//! global map by least squares (the network's output is only relative: scale and offset are free)
//! and the aligned tiles are blended with raised-cosine feathers. The global pass keeps the
//! large-scale ordering right; the tiles add detail the 518 px pass cannot resolve.
//!
//! A tile that has no structure (its depth is flat) cannot be fitted, so it is left out and the
//! pixels it covers keep the global depth.

use craft_ai_core::AiError;

use super::backend;
use super::prep::{DepthMap, MODEL_SIZE, check_dims, postprocess};

/// Most tiles a call may use (the runner caps the working resolution accordingly).
pub(super) const MAX_TILES: u32 = 16;
/// Fraction of a tile that overlaps its neighbour.
const OVERLAP: f64 = 0.25;
/// Allowed range of a tile's fitted scale: outside it the tile is too flat to trust.
const SCALE_RANGE: (f32, f32) = (0.02, 50.0);
/// Below this weighted variance a tile has no usable structure to fit, and is left out.
const MIN_VARIANCE: f64 = 1e-8;

/// A pixel rectangle `[x0, x1) x [y0, y1)` of the working picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TileRect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl TileRect {
    pub fn width(&self) -> u32 {
        self.x1.saturating_sub(self.x0)
    }

    pub fn height(&self) -> u32 {
        self.y1.saturating_sub(self.y0)
    }
}

/// Tile count and length along one axis of `len` pixels when each tile should be about `target`
/// long and at most `max_n` tiles fit: `(n, tile_len)`.
fn axis_plan(len: u32, target: u32, max_n: u32) -> (u32, u32) {
    if len <= target || max_n <= 1 {
        return (1, len);
    }
    let stride = f64::from(target) * (1.0 - OVERLAP);
    let want = ((f64::from(len) - f64::from(target)) / stride).ceil() as u32 + 1;
    let n = want.clamp(1, max_n);
    if n == 1 {
        return (1, len);
    }
    // n tiles of length t overlapping by OVERLAP cover len: t * (1 + (n - 1) * (1 - OVERLAP)) = len.
    let t = (f64::from(len) / (1.0 + f64::from(n - 1) * (1.0 - OVERLAP))).ceil() as u32;
    (n, t.clamp(1, len))
}

/// The grid of overlapping tiles for a `w` x `h` picture: about [`MODEL_SIZE`] px each, 25 %
/// overlap, at most `max_tiles` (clamped to 1..=[`MAX_TILES`]). One tile (the whole picture) means
/// no tiling.
pub(super) fn plan_tiles(w: u32, h: u32, max_tiles: u32) -> Vec<TileRect> {
    let max_tiles = max_tiles.clamp(1, MAX_TILES);
    let (mut nx, mut tw) = axis_plan(w, MODEL_SIZE, max_tiles);
    let (mut ny, mut th) = axis_plan(h, MODEL_SIZE, max_tiles);
    // Too many: coarsen the axis with more tiles until the grid fits.
    while nx * ny > max_tiles {
        if nx >= ny {
            (nx, tw) = axis_plan(w, MODEL_SIZE, nx - 1);
        } else {
            (ny, th) = axis_plan(h, MODEL_SIZE, ny - 1);
        }
    }
    let starts = |len: u32, n: u32, t: u32| -> Vec<u32> {
        if n <= 1 {
            return vec![0];
        }
        let span = f64::from(len.saturating_sub(t));
        (0..n)
            .map(|i| (span * f64::from(i) / f64::from(n - 1)).round() as u32)
            .collect()
    };
    let (xs, ys) = (starts(w, nx, tw), starts(h, ny, th));
    let mut out = Vec::with_capacity(xs.len() * ys.len());
    for &y0 in &ys {
        for &x0 in &xs {
            out.push(TileRect {
                x0,
                y0,
                x1: (x0 + tw).min(w),
                y1: (y0 + th).min(h),
            });
        }
    }
    out
}

/// Bilinear resize of one plane (pixel centres aligned), turning each value with `map` as it is
/// written, so a caller that wants another type never holds a second full-size plane.
pub(super) fn resample_plane_with<T>(
    src: &[f32],
    (sw, sh): (usize, usize),
    (dw, dh): (usize, usize),
    map: impl Fn(f32) -> T,
) -> Vec<T> {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || src.len() < sw * sh {
        let n = dw.saturating_mul(dh);
        return (0..n).map(|_| map(0.0)).collect();
    }
    let at = |x: usize, y: usize| src.get(y * sw + x).copied().unwrap_or(0.0);
    let (fx, fy) = (sw as f32 / dw as f32, sh as f32 / dh as f32);
    let xs: Vec<(usize, usize, f32)> = (0..dw)
        .map(|x| {
            let u = ((x as f32 + 0.5) * fx - 0.5).clamp(0.0, (sw - 1) as f32);
            let x0 = u.floor() as usize;
            (x0, (x0 + 1).min(sw - 1), u - x0 as f32)
        })
        .collect();
    let mut out = Vec::with_capacity(dw * dh);
    for y in 0..dh {
        let v = ((y as f32 + 0.5) * fy - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = v.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let wy = v - y0 as f32;
        out.extend(xs.iter().map(|&(x0, x1, wx)| {
            let a = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * wx;
            let b = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * wx;
            map(a + (b - a) * wy)
        }));
    }
    out
}

/// Raised-cosine weight along one axis for a tile `[a, b)` of a picture `len` long: 1 in the
/// interior, ramping to ~0 at the tile's edges that face a neighbour, and 1 right up to the
/// picture border (nothing overlaps there).
fn axis_weights(a: u32, b: u32, len: u32) -> Vec<f32> {
    let ramp = (f64::from(b - a) * OVERLAP).round().max(1.0);
    (a..b)
        .map(|i| {
            let p = f64::from(i) + 0.5;
            let left = if a > 0 {
                p - f64::from(a)
            } else {
                f64::INFINITY
            };
            let right = if b < len {
                f64::from(b) - p
            } else {
                f64::INFINITY
            };
            let d = left.min(right);
            if d >= ramp {
                1.0
            } else {
                (0.5 - 0.5 * (std::f64::consts::PI * d / ramp).cos()) as f32
            }
        })
        .collect()
}

/// Weighted least squares `a * t + b ~ g` (weights `w`): `(a, b)`, the scale kept positive and
/// bounded. `None` when the tile has no structure to fit (no weight, or a variance too small to
/// give a scale): such a tile must not shift or flatten the global map, so the caller skips it.
fn fit_affine(t: &[f32], g: &[f32], w: &[f32]) -> Option<(f32, f32)> {
    let (mut sw, mut st, mut sg, mut stt, mut stg) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for ((&t, &g), &w) in t.iter().zip(g).zip(w) {
        let (t, g, w) = (f64::from(t), f64::from(g), f64::from(w));
        sw += w;
        st += w * t;
        sg += w * g;
        stt += w * t * t;
        stg += w * t * g;
    }
    if sw <= 0.0 {
        return None;
    }
    let (mt, mg) = (st / sw, sg / sw);
    let var = stt / sw - mt * mt;
    if var <= MIN_VARIANCE {
        return None;
    }
    let cov = stg / sw - mt * mg;
    let a = (cov / var) as f32;
    if !a.is_finite() {
        return None;
    }
    let a = a.clamp(SCALE_RANGE.0, SCALE_RANGE.1);
    let b = (mg - f64::from(a) * mt) as f32;
    b.is_finite().then_some((a, b))
}

/// Whether `map` is exactly the size of `rect` and holds only finite values.
fn tile_matches(rect: &TileRect, map: &DepthMap, (w, h): (u32, u32)) -> bool {
    let (tw, th) = (rect.width() as usize, rect.height() as usize);
    tw != 0
        && th != 0
        && rect.x1 <= w
        && rect.y1 <= h
        && map.width as usize == tw
        && map.height as usize == th
        && map.data.len() == tw * th
        && map.data.iter().all(|v| v.is_finite())
}

/// Blends per-tile depth maps into the global map's frame. `global` is the whole working picture;
/// each tile is `(rect, map)` with `map` exactly `rect`-sized. Every tile with structure is
/// fitted to the global map by weighted least squares and blended with separable raised-cosine
/// feathers. A tile without structure is skipped, and pixels no usable tile covers keep the
/// global value. The result is min-max normalised to 0..1.
pub(super) fn merge_tiles(
    global: &DepthMap,
    tiles: &[(TileRect, DepthMap)],
) -> Result<DepthMap, AiError> {
    let (w, h) = (global.width, global.height);
    check_dims(w, h)?;
    let (wu, hu) = (w as usize, h as usize);
    if global.data.len() != wu * hu || global.data.iter().any(|v| !v.is_finite()) {
        return Err(AiError::invalid("The global depth map is invalid."));
    }
    let mut num = vec![0.0f32; wu * hu];
    let mut den = vec![0.0f32; wu * hu];
    for (r, map) in tiles {
        if !tile_matches(r, map, (w, h)) {
            return Err(AiError::invalid(
                "A tile's depth map does not match its rectangle.",
            ));
        }
        let (tw, th) = (r.width() as usize, r.height() as usize);
        let (wx, wy) = (axis_weights(r.x0, r.x1, w), axis_weights(r.y0, r.y1, h));
        let mut g = Vec::with_capacity(tw * th);
        let mut wt = Vec::with_capacity(tw * th);
        for (j, &wyv) in wy.iter().enumerate() {
            let row = (r.y0 as usize + j) * wu + r.x0 as usize;
            g.extend(global.data.get(row..row + tw).unwrap_or(&[]));
            wt.extend(wx.iter().map(|&wxv| wxv * wyv));
        }
        let Some((a, b)) = fit_affine(&map.data, &g, &wt) else {
            continue;
        };
        for j in 0..th {
            let row = (r.y0 as usize + j) * wu + r.x0 as usize;
            for i in 0..tw {
                let k = j * tw + i;
                let (Some(&t), Some(&wv)) = (map.data.get(k), wt.get(k)) else {
                    continue;
                };
                if let (Some(n), Some(d)) = (num.get_mut(row + i), den.get_mut(row + i)) {
                    *n += wv * (a * t + b);
                    *d += wv;
                }
            }
        }
    }
    let merged: Vec<f32> = num
        .iter()
        .zip(&den)
        .zip(&global.data)
        .map(|((&n, &d), &g)| if d > 1e-6 { n / d } else { g })
        .collect();
    postprocess(&merged, w, h)
}

/// An estimator's output must be a well-formed, finite map.
fn check_map(m: &DepthMap) -> Result<(), AiError> {
    if m.width == 0
        || m.height == 0
        || m.data.len() != m.width as usize * m.height as usize
        || m.data.iter().any(|v| !v.is_finite())
    {
        return Err(backend("The depth model gave a map that cannot be used."));
    }
    Ok(())
}

/// Crops `rect` from interleaved RGB of width `w`.
fn crop_rgb(rgb: &[f32], w: u32, rect: &TileRect) -> Vec<f32> {
    let (wu, tw) = (w as usize, rect.width() as usize);
    let mut out = Vec::with_capacity(tw * rect.height() as usize * 3);
    for y in rect.y0..rect.y1 {
        let start = (y as usize * wu + rect.x0 as usize) * 3;
        out.extend(rgb.get(start..start + tw * 3).unwrap_or(&[]));
    }
    out
}

/// A per-picture estimator: interleaved RGB (0..1), width, height to a depth map of any size.
pub(super) type TileEstimator<'a> = dyn Fn(&[f32], u32, u32) -> Result<DepthMap, AiError> + 'a;

/// The tiled pipeline over any per-picture estimator (`estimate(rgb, w, h)` returns a map of any
/// size, larger = nearer). `rgb` is interleaved 0..1 at the working resolution; the result is at
/// that resolution. `step` gets the fraction done before each estimate (one global pass plus one
/// per tile) and returns an error, such as Cancel, to stop.
pub(super) fn estimate_tiled(
    estimate: &TileEstimator<'_>,
    rgb: &[f32],
    (w, h): (u32, u32),
    max_tiles: u32,
    step: &dyn Fn(f32) -> Result<(), AiError>,
) -> Result<DepthMap, AiError> {
    check_dims(w, h)?;
    if rgb.len() != w as usize * h as usize * 3 {
        return Err(AiError::invalid(format!(
            "expected {} values, got {}",
            w as usize * h as usize * 3,
            rgb.len()
        )));
    }
    let tiles = plan_tiles(w, h, max_tiles);
    let units = (tiles.len() + 1) as f32;
    step(0.0)?;
    let g = estimate(rgb, w, h)?;
    check_map(&g)?;
    let data = resample_plane_with(
        &g.data,
        (g.width as usize, g.height as usize),
        (w as usize, h as usize),
        |v| v,
    );
    let global = postprocess(&data, w, h)?;
    if tiles.len() <= 1 {
        return Ok(global);
    }
    let mut done = Vec::with_capacity(tiles.len());
    for (i, r) in tiles.iter().enumerate() {
        step((i + 1) as f32 / units)?;
        let (tw, th) = (r.width(), r.height());
        let m = estimate(&crop_rgb(rgb, w, r), tw, th)?;
        check_map(&m)?;
        let data = resample_plane_with(
            &m.data,
            (m.width as usize, m.height as usize),
            (tw as usize, th as usize),
            |v| v,
        );
        done.push((
            *r,
            DepthMap {
                width: tw,
                height: th,
                data,
            },
        ));
    }
    merge_tiles(&global, &done)
}

#[cfg(test)]
mod tests;
