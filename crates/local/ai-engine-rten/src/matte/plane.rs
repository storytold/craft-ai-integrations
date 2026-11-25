//! Single-channel float images and the filters the cutout needs (box blur, square dilate and
//! erode, tiling), plus bilinear resampling. All are running sums, linear in the pixel count.

pub trait Lerp: Copy + Default {
    fn mix(a: Self, b: Self, t: f32) -> Self;
}

impl Lerp for f32 {
    fn mix(a: Self, b: Self, t: f32) -> Self {
        a + (b - a) * t
    }
}

impl Lerp for [f32; 3] {
    fn mix(a: Self, b: Self, t: f32) -> Self {
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    }
}

/// Resamples a `w` × `h` image (row-major) to `nw` × `nh`, pixel centres aligned. Empty sizes
/// or a short `src` give an image of defaults.
pub fn bilinear<T: Lerp>(src: &[T], w: usize, h: usize, nw: usize, nh: usize) -> Vec<T> {
    let mut out = vec![T::default(); nw * nh];
    if w == 0 || h == 0 || src.len() < w * h {
        return out;
    }
    let (sx, sy) = (w as f32 / nw.max(1) as f32, h as f32 / nh.max(1) as f32);
    let at = |x: usize, y: usize| src[y * w + x];
    for y in 0..nh {
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (h - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(h - 1);
        for x in 0..nw {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (w - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let x1 = (x0 + 1).min(w - 1);
            let top = T::mix(at(x0, y0), at(x1, y0), tx);
            let bottom = T::mix(at(x0, y1), at(x1, y1), tx);
            out[y * nw + x] = T::mix(top, bottom, ty);
        }
    }
    out
}

/// A `w` × `h` single-channel image, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct Plane {
    pub w: usize,
    pub h: usize,
    pub v: Vec<f32>,
}

impl Plane {
    pub fn new(w: usize, h: usize, fill: f32) -> Self {
        Self {
            w,
            h,
            v: vec![fill; w * h],
        }
    }

    pub fn from_fn(w: usize, h: usize, f: impl Fn(usize) -> f32) -> Self {
        Self {
            w,
            h,
            v: (0..w * h).map(f).collect(),
        }
    }

    pub fn resized(&self, w: usize, h: usize) -> Self {
        if (w, h) == (self.w, self.h) {
            return self.clone();
        }
        Self {
            w,
            h,
            v: bilinear(&self.v, self.w, self.h, w, h),
        }
    }

    /// Box mean of radius `r` (window 2r+1), edges clamped.
    pub fn box_blur(&self, r: usize) -> Self {
        let mut out = self.clone();
        out.box_blur_in_place(r);
        out
    }

    /// [`Plane::box_blur`] without a second plane: one line of scratch at a time.
    pub fn box_blur_in_place(&mut self, r: usize) {
        if r == 0 {
            return;
        }
        blur_1d(&mut self.v, self.w, self.h, r, true);
        blur_1d(&mut self.v, self.w, self.h, r, false);
    }
}

/// Running-sum box mean along rows (`horizontal`) or columns, edges clamped, in place. An empty
/// axis is left as it is.
fn blur_1d(v: &mut [f32], w: usize, h: usize, r: usize, horizontal: bool) {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    if len == 0 || v.len() < w * h {
        return;
    }
    let at = |line: usize, i: usize| {
        if horizontal {
            line * w + i
        } else {
            i * w + line
        }
    };
    let k = (2 * r + 1) as f64;
    let mut src = vec![0.0f32; len];
    for line in 0..lines {
        for (i, s) in src.iter_mut().enumerate() {
            *s = v[at(line, i)];
        }
        let get = |i: isize| f64::from(src[i.clamp(0, len as isize - 1) as usize]);
        let mut sum: f64 = (-(r as isize)..=r as isize).map(get).sum();
        for i in 0..len {
            v[at(line, i)] = (sum / k) as f32;
            sum += get(i as isize + r as isize + 1) - get(i as isize - r as isize);
        }
    }
}

/// A boolean mask as 0/1 floats, for the filters above.
pub fn mask(w: usize, h: usize, f: impl Fn(usize) -> bool) -> Plane {
    Plane::from_fn(w, h, |i| if f(i) { 1.0 } else { 0.0 })
}

/// Square dilation of a 0/1 mask by `r`: any set pixel within the window. Reuses the mask.
pub fn dilate(m: Plane, r: usize) -> Plane {
    threshold(m, r, 1e-6)
}

/// Square erosion of a 0/1 mask by `r`: every pixel within the window set (edges clamped).
/// Reuses the mask.
pub fn erode(m: Plane, r: usize) -> Plane {
    threshold(m, r, 1.0 - 1e-6)
}

fn threshold(mut m: Plane, r: usize, above: f32) -> Plane {
    m.box_blur_in_place(r);
    for x in &mut m.v {
        *x = if *x > above { 1.0 } else { 0.0 };
    }
    m
}

/// A rectangle inside a plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tile {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

/// Tile origins along a side of `len`: `size` wide, overlapping by `overlap`, the last flush with
/// the end. One tile when the side fits.
pub fn starts(len: usize, size: usize, overlap: usize) -> Vec<usize> {
    if len <= size {
        return vec![0];
    }
    let step = size.saturating_sub(overlap).max(1);
    let mut v: Vec<usize> = (0..=len - size).step_by(step).collect();
    if v.last().is_some_and(|l| l + size < len) {
        v.push(len - size);
    }
    v
}

/// The tiles covering a `w` × `h` plane.
pub fn tiles(w: usize, h: usize, size: usize, overlap: usize) -> Vec<Tile> {
    let mut out = Vec::new();
    for y in starts(h, size, overlap) {
        for x in starts(w, size, overlap) {
            out.push(Tile {
                x,
                y,
                w: size.min(w - x),
                h: size.min(h - y),
            });
        }
    }
    out
}

/// A tile's blending weight in a `w` × `h` picture: rises over `ramp` pixels from each tile edge
/// that lies inside the picture, so overlapping tiles fade into each other instead of meeting at
/// a seam. An edge on the picture's own border has no neighbour to fade into and stays at full
/// weight.
pub fn ramp_weight(t: Tile, ramp: usize, w: usize, h: usize) -> Plane {
    let ramp = ramp.max(1);
    let edge = |i: usize, n: usize, start: usize, len: usize| {
        let mut r = ramp;
        if start > 0 {
            r = r.min(i + 1);
        }
        if start + n < len {
            r = r.min(n - i);
        }
        r as f32
    };
    Plane::from_fn(t.w, t.h, |i| {
        edge(i % t.w, t.w, t.x, w).min(edge(i / t.w, t.h, t.y, h))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_keeps_constants_and_spreads_a_dot() {
        let c = Plane::new(9, 7, 0.4).box_blur(3);
        assert!(c.v.iter().all(|x| (x - 0.4).abs() < 1e-5));
        let mut d = Plane::new(9, 9, 0.0);
        d.v[4 * 9 + 4] = 9.0;
        let b = d.box_blur(1);
        assert!(
            (b.v[4 * 9 + 4] - 1.0).abs() < 1e-5
                && (b.v[3 * 9 + 3] - 1.0).abs() < 1e-5
                && b.v[0] == 0.0
        );
    }

    #[test]
    fn dilate_and_erode_grow_and_shrink_a_square() {
        let m = mask(10, 10, |i| {
            (3..7).contains(&(i % 10)) && (3..7).contains(&(i / 10))
        });
        let on = |p: &Plane| p.v.iter().filter(|x| **x > 0.5).count();
        assert_eq!(on(&dilate(m.clone(), 1)), 36);
        assert_eq!(on(&erode(m, 1)), 4);
    }

    #[test]
    fn tiles_cover_with_overlap_and_end_flush() {
        assert_eq!(starts(500, 1024, 96), vec![0]);
        assert_eq!(starts(2000, 1024, 96), vec![0, 928, 976]);
        let t = tiles(2000, 900, 1024, 96);
        assert_eq!(t.len(), 3);
        assert!(t.iter().all(|t| t.x + t.w <= 2000 && t.y + t.h <= 900));
    }

    #[test]
    fn a_ramp_fades_only_into_neighbouring_tiles() {
        // The left tile of two across a 20 × 10 picture: its right edge meets the next tile, its
        // other edges are the picture's border.
        let t = Tile {
            x: 0,
            y: 0,
            w: 12,
            h: 10,
        };
        let w = ramp_weight(t, 3, 20, 10);
        assert_eq!(w.v[0], 3.0, "the picture's corner is at full weight");
        assert_eq!(w.v[5 * 12 + 5], 3.0);
        assert_eq!(w.v[5 * 12 + 11], 1.0, "the inner edge fades");
        // A tile that is the whole picture is at full weight everywhere.
        let whole = Tile {
            x: 0,
            y: 0,
            w: 6,
            h: 4,
        };
        assert!(ramp_weight(whole, 3, 6, 4).v.iter().all(|x| *x == 3.0));
    }

    #[test]
    fn blurring_an_empty_axis_is_not_a_panic() {
        assert!(Plane::new(0, 5, 0.0).box_blur(2).v.is_empty());
        assert!(Plane::new(5, 0, 0.0).box_blur(2).v.is_empty());
    }
}
