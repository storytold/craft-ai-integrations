//! The colour guided filter (K. He, J. Sun and X. Tang, "Guided Image Filtering", PAMI 2013) that
//! brings fine detail back into an enlarged matte, run only near the matte's edge.
//!
//! The filter only changes pixels within `2r` of a non-constant part of the matte, so it runs tile
//! by tile on tiles near the edge only and gives the same result as filtering the whole picture:
//! memory follows the edge length, not the area.

use craft_ai_core::AiError;
use craft_ai_engine::RunContext;

use super::Picture;

/// Output tile size. At least `2r`, so a tile's neighbours cover its halo.
const TILE: usize = 256;

/// `work` on every item, spread over the processors with scoped threads, answers in item order. A
/// thread that cannot start or that panics leaves `None` for its items.
fn in_parallel<T: Sync, R: Send>(
    items: &[T],
    work: &(dyn Fn(&T) -> Option<R> + Sync),
) -> Vec<Option<R>> {
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .clamp(1, 32);
    let per = items.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks(per)
            .map(|chunk| {
                let run = move || chunk.iter().map(work).collect::<Vec<_>>();
                (
                    chunk.len(),
                    std::thread::Builder::new().spawn_scoped(scope, run),
                )
            })
            .collect();
        let mut out = Vec::with_capacity(items.len());
        for (len, handle) in handles {
            match handle.map(std::thread::ScopedJoinHandle::join) {
                Ok(Ok(part)) => out.extend(part),
                _ => out.extend((0..len).map(|_| None)),
            }
        }
        out
    })
}

/// Mean of `src` (`w × h`) over the `(2r + 1)²` window around each pixel, clipped at the edges.
pub(super) fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0.0f32; w * h];
    let mut pre = vec![0.0f64; w + 1];
    for (row, out) in src
        .chunks_exact(w.max(1))
        .zip(tmp.chunks_exact_mut(w.max(1)))
    {
        for (x, v) in row.iter().enumerate() {
            pre[x + 1] = pre[x] + *v as f64;
        }
        for (x, o) in out.iter_mut().enumerate() {
            let (lo, hi) = (x.saturating_sub(r), (x + r + 1).min(w));
            *o = ((pre[hi] - pre[lo]) / (hi - lo) as f64) as f32;
        }
    }
    let mut col = vec![0.0f64; (h + 1) * w];
    for y in 0..h {
        for x in 0..w {
            col[(y + 1) * w + x] = col[y * w + x] + tmp[y * w + x] as f64;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let (lo, hi) = (y.saturating_sub(r), (y + r + 1).min(h));
        let n = (hi - lo) as f64;
        for x in 0..w {
            out[y * w + x] = ((col[hi * w + x] - col[lo * w + x]) / n) as f32;
        }
    }
    out
}

/// Guided filter with a colour guide (He et al., Eq. 19–21: `a_k = (Σ_k + εU)⁻¹ cov_k(I, p)`).
/// `guide` and `p` are both `w × h`.
pub(super) fn filter(
    guide: &[[f32; 3]],
    p: &[f32],
    w: usize,
    h: usize,
    r: usize,
    eps: f32,
) -> Vec<f32> {
    let n = w * h;
    let ch = |c: usize| -> Vec<f32> { guide.iter().map(|q| q[c]).collect() };
    let chans = [ch(0), ch(1), ch(2)];
    let mean = |v: &[f32]| box_mean(v, w, h, r);
    let product =
        |a: &[f32], b: &[f32]| -> Vec<f32> { a.iter().zip(b).map(|(x, y)| x * y).collect() };
    let m: Vec<Vec<f32>> = chans.iter().map(|c| mean(c)).collect();
    let mp = mean(p);
    let mip: Vec<Vec<f32>> = chans.iter().map(|c| mean(&product(c, p))).collect();
    let pairs = [(0, 0), (0, 1), (0, 2), (1, 1), (1, 2), (2, 2)];
    let mii: Vec<Vec<f32>> = pairs
        .iter()
        .map(|&(a, b)| mean(&product(&chans[a], &chans[b])))
        .collect();
    let mut av = [vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]];
    let mut bv = vec![0.0f32; n];
    let e = eps as f64;
    for i in 0..n {
        let mu = [m[0][i] as f64, m[1][i] as f64, m[2][i] as f64];
        let s = |k: usize, a: usize, b: usize| mii[k][i] as f64 - mu[a] * mu[b];
        let (rr, rg, rb) = (s(0, 0, 0) + e, s(1, 0, 1), s(2, 0, 2));
        let (gg, gb, bb) = (s(3, 1, 1) + e, s(4, 1, 2), s(5, 2, 2) + e);
        let cov = [
            mip[0][i] as f64 - mu[0] * mp[i] as f64,
            mip[1][i] as f64 - mu[1] * mp[i] as f64,
            mip[2][i] as f64 - mu[2] * mp[i] as f64,
        ];
        // Inverse of the symmetric 3×3 matrix.
        let i00 = gg * bb - gb * gb;
        let i01 = gb * rb - rg * bb;
        let i02 = rg * gb - gg * rb;
        let i11 = rr * bb - rb * rb;
        let i12 = rb * rg - rr * gb;
        let i22 = rr * gg - rg * rg;
        let det = rr * i00 + rg * i01 + rb * i02;
        let (a0, a1, a2) = if det.abs() < 1e-30 {
            (0.0, 0.0, 0.0)
        } else {
            (
                (i00 * cov[0] + i01 * cov[1] + i02 * cov[2]) / det,
                (i01 * cov[0] + i11 * cov[1] + i12 * cov[2]) / det,
                (i02 * cov[0] + i12 * cov[1] + i22 * cov[2]) / det,
            )
        };
        av[0][i] = a0 as f32;
        av[1][i] = a1 as f32;
        av[2][i] = a2 as f32;
        bv[i] = (mp[i] as f64 - a0 * mu[0] - a1 * mu[1] - a2 * mu[2]) as f32;
    }
    let ma: Vec<Vec<f32>> = av.iter().map(|a| mean(a)).collect();
    let mb = mean(&bv);
    (0..n)
        .map(|i| ma[0][i] * chans[0][i] + ma[1][i] * chans[1][i] + ma[2][i] * chans[2][i] + mb[i])
        .collect()
}

/// A rectangle of the picture, `x0..x1` × `y0..y1`.
#[derive(Clone, Copy)]
struct Area {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl Area {
    fn width(&self) -> usize {
        self.x1 - self.x0
    }
}

/// `mask = max(guided_filter(picture, mask), mask)`, computed only on tiles within `2r` of a
/// non-constant part of the mask (elsewhere the filter returns its input exactly).
pub(super) fn refine(
    picture: Picture<'_>,
    mask: &mut [u8],
    r: usize,
    eps: f32,
    ctx: &RunContext,
) -> Result<(), AiError> {
    let (w, h) = (picture.width, picture.height);
    let halo = 2 * r;
    let (tw, th) = (w.div_ceil(TILE), h.div_ceil(TILE));
    let tile = |tx: usize, ty: usize| Area {
        x0: tx * TILE,
        y0: ty * TILE,
        x1: ((tx + 1) * TILE).min(w),
        y1: ((ty + 1) * TILE).min(h),
    };
    // Per-tile value range; a tile needs filtering when the range over its neighbourhood (which
    // covers the 2r halo, as 2r ≤ TILE) is not a single value.
    let mut range = vec![(u8::MAX, u8::MIN); tw * th];
    for ty in 0..th {
        for tx in 0..tw {
            let t = tile(tx, ty);
            let mut rg = (u8::MAX, u8::MIN);
            for y in t.y0..t.y1 {
                for v in mask.get(y * w + t.x0..y * w + t.x1).unwrap_or(&[]) {
                    rg = (rg.0.min(*v), rg.1.max(*v));
                }
            }
            range[ty * tw + tx] = rg;
        }
    }
    let mut active = Vec::new();
    for ty in 0..th {
        for tx in 0..tw {
            let mut rg = (u8::MAX, u8::MIN);
            for ny in ty.saturating_sub(1)..=(ty + 1).min(th - 1) {
                for nx in tx.saturating_sub(1)..=(tx + 1).min(tw - 1) {
                    let q = range[ny * tw + nx];
                    rg = (rg.0.min(q.0), rg.1.max(q.1));
                }
            }
            if rg.0 != rg.1 {
                active.push(tile(tx, ty));
            }
        }
    }

    // Each tile reads the mask as it was before any tile is written, so tiles are independent and
    // run on every processor at once.
    let src: &[u8] = mask;
    let one = |t: &Area| -> Option<Vec<u8>> {
        if ctx.is_cancelled() {
            return None;
        }
        let rd = Area {
            x0: t.x0.saturating_sub(halo),
            y0: t.y0.saturating_sub(halo),
            x1: (t.x1 + halo).min(w),
            y1: (t.y1 + halo).min(h),
        };
        let (rw, rh) = (rd.width(), rd.y1 - rd.y0);
        let guide = picture.rgb(rd.x0, rd.y0, rd.x1, rd.y1);
        let mut pm = Vec::with_capacity(rw * rh);
        for y in rd.y0..rd.y1 {
            pm.extend(
                src.get(y * w + rd.x0..y * w + rd.x1)?
                    .iter()
                    .map(|v| *v as f32 / 255.0),
            );
        }
        let g = filter(&guide, &pm, rw, rh, r, eps);
        let mut out = Vec::with_capacity(t.width() * (t.y1 - t.y0));
        for y in t.y0..t.y1 {
            let at = (y - rd.y0) * rw + (t.x0 - rd.x0);
            let gs = g.get(at..at + t.width())?;
            let ps = pm.get(at..at + t.width())?;
            out.extend(
                gs.iter()
                    .zip(ps)
                    .map(|(gv, pv)| (gv.clamp(0.0, 1.0).max(*pv) * 255.0).round() as u8),
            );
        }
        Some(out)
    };
    let results = in_parallel(&active, &one);
    ctx.check_cancelled()?;
    for (t, out) in active.iter().zip(results) {
        let Some(out) = out else { continue };
        for (y, row) in (t.y0..t.y1).zip(out.chunks_exact(t.width().max(1))) {
            if let Some(dst) = mask.get_mut(y * w + t.x0..y * w + t.x1) {
                dst.copy_from_slice(row);
            }
        }
    }
    Ok(())
}
