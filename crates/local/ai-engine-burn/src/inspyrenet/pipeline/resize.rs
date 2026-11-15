//! Getting the picture into the network's input and the matte back out: reflect padding and
//! antialiased Lanczos-3, as Gyre's resize (ResizeRight) does.

use craft_ai_core::AiError;
use craft_ai_engine::RunContext;

use super::{Picture, Plan};

/// Source rows read at once while reducing a large picture.
const STRIP: usize = 64;

/// The network input: RGB in `0..=1`, row by row.
pub struct Input {
    pub width: usize,
    pub height: usize,
    pub px: Vec<[f32; 3]>,
}

/// Mirror index `i` into `0..n` without repeating the edge sample (PyTorch's `reflect`), for any
/// offset: padding wider than the picture keeps bouncing.
pub(super) fn reflect(i: i64, n: usize) -> usize {
    if n <= 1 {
        return 0;
    }
    let period = 2 * (n as i64 - 1);
    let m = i.rem_euclid(period);
    (if m < n as i64 { m } else { period - m }) as usize
}

/// The Lanczos-3 kernel.
fn lanczos(x: f64) -> f64 {
    let x = x.abs();
    if x < 1e-9 {
        1.0
    } else if x < 3.0 {
        let px = std::f64::consts::PI * x;
        3.0 * px.sin() * (px / 3.0).sin() / (px * px)
    } else {
        0.0
    }
}

/// Lanczos-3 taps of output sample `o` on an axis scaled by `scale` (output / input): the first
/// source index and the normalised weights. Pixel centres align, and the kernel widens by
/// 1/scale when reducing (antialiasing).
pub(super) fn lanczos_taps(o: usize, scale: f64) -> (i64, Vec<f32>) {
    let stretch = if scale < 1.0 { 1.0 / scale } else { 1.0 };
    let u = (o as f64 + 0.5) / scale - 0.5;
    let lo = (u - 3.0 * stretch).ceil() as i64;
    let hi = (u + 3.0 * stretch).floor() as i64;
    let w: Vec<f64> = (lo..=hi)
        .map(|i| lanczos((i as f64 - u) / stretch))
        .collect();
    let sum: f64 = w.iter().sum();
    let k = if sum.abs() > 1e-12 { 1.0 / sum } else { 1.0 };
    (lo, w.into_iter().map(|v| (v * k) as f32).collect())
}

/// `picture` reflect-padded into the network input (the full-resolution path).
pub(super) fn pad_input(picture: Picture<'_>, p: &Plan) -> Input {
    let (w, h) = (picture.width, picture.height);
    let all = picture.rgb(0, 0, w, h);
    let mut px = Vec::with_capacity(p.in_w * p.in_h);
    for y in 0..p.in_h {
        let sy = reflect(y as i64 - p.top as i64, h);
        for x in 0..p.in_w {
            let sx = reflect(x as i64 - p.left as i64, w);
            px.push(all.get(sy * w + sx).copied().unwrap_or([0.0; 3]));
        }
    }
    Input {
        width: p.in_w,
        height: p.in_h,
        px,
    }
}

/// The network input of a large picture: reflect-padded to a square (at the left and top) and
/// reduced to `in_w × in_h` with antialiased Lanczos-3. Source rows stream through in strips;
/// each is filtered horizontally and added to the output rows it feeds.
pub(super) fn reduce_input(
    picture: Picture<'_>,
    p: &Plan,
    ctx: &RunContext,
) -> Result<Input, AiError> {
    let (w, h) = (picture.width, picture.height);
    // A tap past the square reflects at the square's edge first (the resize pads the padded
    // square), then lands in the picture through the padding's own reflection.
    let source = |i: i64, pad: usize, n: usize| reflect(reflect(i, pad + n) as i64 - pad as i64, n);
    let cols: Vec<Vec<(usize, f32)>> = (0..p.in_w)
        .map(|o| {
            let (lo, ws) = lanczos_taps(o, p.scale);
            ws.into_iter()
                .enumerate()
                .map(|(i, wt)| (source(lo + i as i64, p.left, w), wt))
                .collect()
        })
        .collect();
    // For each source row, the output rows it feeds and with what weight.
    let mut feeds: Vec<Vec<(usize, f32)>> = vec![Vec::new(); h];
    for oy in 0..p.in_h {
        let (lo, ws) = lanczos_taps(oy, p.scale);
        for (i, wt) in ws.into_iter().enumerate() {
            if let Some(f) = feeds.get_mut(source(lo + i as i64, p.top, h)) {
                f.push((oy, wt));
            }
        }
    }
    let mut out = vec![[0.0f32; 3]; p.in_w * p.in_h];
    for y0 in (0..h).step_by(STRIP) {
        ctx.check_cancelled()?;
        let y1 = (y0 + STRIP).min(h);
        let strip = picture.rgb(0, y0, w, y1);
        for (src, f) in strip.chunks_exact(w).zip(feeds.get(y0..y1).unwrap_or(&[])) {
            let row: Vec<[f32; 3]> = cols
                .iter()
                .map(|taps| {
                    let mut acc = [0.0f32; 3];
                    for (c, wt) in taps {
                        let s = src.get(*c).copied().unwrap_or([0.0; 3]);
                        for k in 0..3 {
                            acc[k] += wt * s[k];
                        }
                    }
                    acc
                })
                .collect();
            for &(oy, wt) in f {
                let dst = out
                    .get_mut(oy * p.in_w..(oy + 1) * p.in_w)
                    .unwrap_or(&mut []);
                for (d, s) in dst.iter_mut().zip(&row) {
                    for k in 0..3 {
                        d[k] += wt * s[k];
                    }
                }
            }
        }
        ctx.progress((10 * y1 / h.max(1)) as u8);
    }
    for px in &mut out {
        *px = px.map(|v| v.clamp(0.0, 1.0));
    }
    Ok(Input {
        width: p.in_w,
        height: p.in_h,
        px: out,
    })
}

/// The full-resolution `w`-wide matte of a large picture: the prediction enlarged back to the
/// padded square with Lanczos-3 and cropped, one output row at a time (vertical taps first, so
/// nothing larger than a prediction row is held per row).
pub(super) fn expand_matte(pred: &[f32], p: &Plan, w: usize, mask: &mut [u8]) {
    let up = 1.0 / p.scale;
    let cols: Vec<(i64, Vec<f32>)> = (0..w).map(|x| lanczos_taps(x + p.left, up)).collect();
    let mut line = vec![0.0f32; p.in_w];
    for (y, row) in mask.chunks_exact_mut(w.max(1)).enumerate() {
        let (lo, wy) = lanczos_taps(y + p.top, up);
        line.fill(0.0);
        for (i, wt) in wy.iter().enumerate() {
            let r = reflect(lo + i as i64, p.in_h);
            let src = pred.get(r * p.in_w..(r + 1) * p.in_w).unwrap_or(&[]);
            for (l, s) in line.iter_mut().zip(src) {
                *l += wt * s;
            }
        }
        for (o, (lo, wx)) in row.iter_mut().zip(&cols) {
            let v: f32 = wx
                .iter()
                .enumerate()
                .map(|(i, wt)| {
                    wt * line
                        .get(reflect(lo + i as i64, p.in_w))
                        .copied()
                        .unwrap_or(0.0)
                })
                .sum();
            *o = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
}
