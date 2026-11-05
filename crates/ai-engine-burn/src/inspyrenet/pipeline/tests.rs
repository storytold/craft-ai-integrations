use std::cell::RefCell;

use craft_ai_core::{AiError, ErrorKind};
use craft_ai_engine::RunContext;

use super::guided::{filter, refine};
use super::resize::{Input, expand_matte, lanczos_taps, reduce_input, reflect};
use super::*;

/// A disc of `inside` colour on an `outside` background, and which pixels are inside.
fn disc(w: usize, h: usize, cx: f32, cy: f32, rad: f32) -> (Vec<u8>, Vec<bool>) {
    let mut bytes = Vec::with_capacity(w * h * 3);
    let mut truth = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let inside = dx * dx + dy * dy <= rad * rad;
            bytes.extend_from_slice(if inside {
                &[230, 76, 51]
            } else {
                &[51, 102, 178]
            });
            truth.push(inside);
        }
    }
    (bytes, truth)
}

fn picture(bytes: &[u8], width: usize, height: usize) -> Picture<'_> {
    Picture {
        width,
        height,
        bytes,
    }
}

/// Intersection over union of the coverage above one half and `truth`.
fn iou(mask: &[u8], truth: &[bool]) -> f64 {
    let (mut both, mut any) = (0usize, 0usize);
    for (m, t) in mask.iter().zip(truth) {
        let m = *m > 127;
        both += usize::from(m && *t);
        any += usize::from(m || *t);
    }
    both as f64 / any.max(1) as f64
}

/// A stand-in network: foreground where the red channel is high. It records the input sizes.
struct Red(RefCell<Vec<(usize, usize)>>);

impl Red {
    fn new() -> Self {
        Self(RefCell::new(Vec::new()))
    }

    fn predict(&self, input: &Input) -> Result<Vec<f32>, AiError> {
        assert_eq!(input.width % ALIGN, 0);
        assert_eq!(input.height % ALIGN, 0);
        self.0.borrow_mut().push((input.width, input.height));
        Ok(input
            .px
            .iter()
            .map(|p| if p[0] > 0.6 { 1.0 } else { 0.0 })
            .collect())
    }
}

fn run(pic: Picture<'_>, shrink: f32, model: &Red) -> Result<Vec<u8>, AiError> {
    matte(
        pic,
        MAX_RES,
        shrink,
        &mut |i| model.predict(i),
        &RunContext::detached(),
    )
}

#[test]
fn plan_pads_small_pictures_to_the_stride_centred() {
    let p = plan(100, 50, MAX_RES).unwrap();
    assert_eq!(
        (p.scale, p.in_w, p.in_h, p.left, p.top),
        (1.0, 128, 64, 14, 7)
    );
    assert!(!p.guided());
    let p = plan(64, 1280, MAX_RES).unwrap();
    assert_eq!((p.in_w, p.in_h, p.left, p.top), (64, 1280, 0, 0));
}

#[test]
fn plan_pads_large_pictures_to_a_square_at_the_left_and_top() {
    let p = plan(4000, 2000, MAX_RES).unwrap();
    assert_eq!(
        (p.scale, p.in_w, p.in_h, p.left, p.top),
        (0.32, 1280, 1280, 0, 2000)
    );
    assert!(p.guided());
    assert!(!plan(2000, 1000, MAX_RES).unwrap().guided());
    assert_eq!(plan(100_000, 3, MAX_RES).unwrap().top, 99_997);
}

#[test]
fn plan_rejects_empty_and_bounds_max_res() {
    assert_eq!(plan(0, 10, MAX_RES), None);
    assert_eq!(plan(10, 0, MAX_RES), None);
    assert_eq!(plan(5000, 5000, 0).unwrap().in_w, ALIGN);
    assert_eq!(plan(5000, 10, 1000).unwrap().in_w, 992);
}

#[test]
fn reflect_mirrors_without_repeating_the_edge() {
    let r: Vec<usize> = (-4..8).map(|i| reflect(i, 3)).collect();
    assert_eq!(r, [0, 1, 2, 1, 0, 1, 2, 1, 0, 1, 2, 1]);
    assert_eq!(reflect(-7, 1), 0);
    assert_eq!(reflect(i64::MIN / 2, 5), reflect(i64::MIN / 2 + 8, 5));
}

#[test]
fn lanczos_taps_are_normalised_and_exact_at_scale_one() {
    for scale in [0.07, 0.32, 0.5, 0.9, 1.7, 4.0] {
        for o in [0, 1, 17, 200] {
            let (_, w) = lanczos_taps(o, scale);
            assert!(
                (w.iter().sum::<f32>() - 1.0).abs() < 1e-5,
                "scale {scale} o {o}"
            );
        }
    }
    assert!(lanczos_taps(10, 0.25).1.len() >= 23);
    assert!(lanczos_taps(10, 3.0).1.len() <= 7);
    let (lo, w) = lanczos_taps(5, 1.0);
    let one: Vec<(i64, f32)> = (lo..).zip(w).filter(|(_, v)| v.abs() > 1e-6).collect();
    assert_eq!(one, [(5, 1.0)]);
}

/// `src` (`sw × sh`, `ch` channels) resized by applying the taps directly, reflecting at its
/// edges: what the streamed versions must equal.
fn naive_resize(
    src: &[f32],
    sw: usize,
    sh: usize,
    ch: usize,
    ow: usize,
    oh: usize,
    scale: f64,
) -> Vec<f32> {
    let mut out = vec![0.0f32; ow * oh * ch];
    for oy in 0..oh {
        let (ly, wy) = lanczos_taps(oy, scale);
        for ox in 0..ow {
            let (lx, wx) = lanczos_taps(ox, scale);
            for (j, a) in wy.iter().enumerate() {
                for (i, b) in wx.iter().enumerate() {
                    let s = reflect(ly + j as i64, sh) * sw + reflect(lx + i as i64, sw);
                    for c in 0..ch {
                        out[(oy * ow + ox) * ch + c] += a * b * src[s * ch + c];
                    }
                }
            }
        }
    }
    out
}

#[test]
fn streamed_reduction_matches_the_padded_square() {
    for (w, h) in [(300usize, 180usize), (300, 40)] {
        let bytes: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                [
                    (x * 255 / w) as u8,
                    (y * 255 / h) as u8,
                    ((x * 7 + y * 13) % 17 * 15) as u8,
                ]
            })
            .collect();
        let pic = picture(&bytes, w, h);
        let p = plan(w, h, 64).unwrap();
        let got = reduce_input(pic, &p, &RunContext::detached()).unwrap();
        let side = w.max(h);
        let square: Vec<f32> = (0..side * side)
            .flat_map(|i| {
                let sx = reflect((i % side) as i64 - p.left as i64, w);
                let sy = reflect((i / side) as i64 - p.top as i64, h);
                let at = (sy * w + sx) * 3;
                [bytes[at], bytes[at + 1], bytes[at + 2]].map(|v| v as f32 / 255.0)
            })
            .collect();
        let want = naive_resize(&square, side, side, 3, 64, 64, p.scale);
        let worst = got
            .px
            .iter()
            .flatten()
            .zip(&want)
            .map(|(a, b)| (a - b.clamp(0.0, 1.0)).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-4, "{w}×{h}: {worst}");
    }
}

#[test]
fn row_by_row_enlargement_matches_the_padded_square() {
    let (w, h) = (300usize, 180usize);
    let p = plan(w, h, 64).unwrap();
    let pred: Vec<f32> = (0..64 * 64)
        .map(|i| (((i % 64) as f32 - 30.0) / 6.0).tanh() * 0.5 + 0.5)
        .collect();
    let mut got = vec![0u8; w * h];
    expand_matte(&pred, &p, w, &mut got);
    let side = w.max(h);
    let full = naive_resize(&pred, 64, 64, 1, side, side, 1.0 / p.scale);
    let worst = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let want = (full[(y + p.top) * side + x + p.left].clamp(0.0, 1.0) * 255.0).round();
            (got[i] as i32 - want as i32).abs()
        })
        .max()
        .unwrap();
    assert!(worst <= 1, "{worst}");
}

#[test]
fn shrink_maps_the_fringe() {
    assert_eq!(shrink_lut(0.0), std::array::from_fn(|v| v as u8));
    let l = shrink_lut(0.2);
    assert_eq!((l[0], l[51], l[153], l[255]), (0, 0, 128, 255));
    assert_eq!(shrink_lut(1.0)[255], 255);
    assert_eq!(shrink_lut(f32::NAN), shrink_lut(0.0));
}

#[test]
fn a_small_picture_runs_at_full_resolution() {
    let (bytes, truth) = disc(200, 150, 100.0, 75.0, 40.0);
    let model = Red::new();
    let mask = run(picture(&bytes, 200, 150), 0.0, &model).unwrap();
    assert_eq!(*model.0.borrow(), [(224, 160)]);
    assert!(iou(&mask, &truth) > 0.98, "iou {}", iou(&mask, &truth));
}

#[test]
fn a_large_picture_is_reduced_and_refined_at_full_resolution() {
    // 2800 × 1400: working scale 0.457, so the guided filter runs.
    let (w, h) = (2800usize, 1400usize);
    let (bytes, truth) = disc(w, h, 1400.0, 700.0, 500.0);
    let model = Red::new();
    let raw = run(picture(&bytes, w, h), 0.0, &model).unwrap();
    assert_eq!(*model.0.borrow(), [(1280, 1280)]);
    // The guided filter snaps the blurry enlarged edge to the picture's hard edge.
    assert!(iou(&raw, &truth) > 0.995, "iou {}", iou(&raw, &truth));
    // The default shrink removes the faint haze the filter windows leave outside the edge.
    let mask = run(picture(&bytes, w, h), SHRINK, &model).unwrap();
    assert_eq!(mask[700 * w + 1400], 255);
    assert_eq!(mask[190 * w + 1400], 0);
    assert!(iou(&mask, &truth) > 0.995);
}

#[test]
fn tiled_refinement_matches_the_whole_picture() {
    let (w, h) = (700usize, 560usize);
    let (bytes, _) = disc(w, h, 330.0, 290.0, 200.0);
    let mut mask: Vec<u8> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            let d = ((x - 340.0).powi(2) + (y - 280.0).powi(2)).sqrt();
            (((210.0 - d) / 12.0).clamp(0.0, 1.0) * 255.0).round() as u8
        })
        .collect();
    let pm: Vec<f32> = mask.iter().map(|v| *v as f32 / 255.0).collect();
    let pic = picture(&bytes, w, h);
    let guide = pic.rgb(0, 0, w, h);
    let whole: Vec<u8> = filter(&guide, &pm, w, h, 32, GUIDED_EPS)
        .iter()
        .zip(&pm)
        .map(|(g, p)| (g.clamp(0.0, 1.0).max(*p) * 255.0).round() as u8)
        .collect();
    refine(pic, &mut mask, 32, GUIDED_EPS, &RunContext::detached()).unwrap();
    let worst = mask
        .iter()
        .zip(&whole)
        .map(|(a, b)| (*a as i32 - *b as i32).abs())
        .max()
        .unwrap();
    assert!(worst <= 1, "tiles differ from the whole picture by {worst}");
}

#[test]
fn a_wrong_answer_from_the_network_is_an_error_and_odd_values_clamp() {
    let (bytes, _) = disc(64, 48, 32.0, 24.0, 12.0);
    let pic = picture(&bytes, 64, 48);
    let ctx = RunContext::detached();
    let answer = |v: Vec<f32>| matte(pic, MAX_RES, 0.0, &mut |_| Ok(v.clone()), &ctx);
    assert_eq!(answer(vec![0.5; 7]).unwrap_err().kind, ErrorKind::Internal);
    assert_eq!(answer(Vec::new()).unwrap_err().kind, ErrorKind::Internal);
    let failed = matte(
        pic,
        MAX_RES,
        0.0,
        &mut |_| Err(AiError::invalid("no")),
        &ctx,
    );
    assert_eq!(failed.unwrap_err().message, "no");
    // NaN is background, +inf foreground. Input 64×64 holds the picture 8 rows down.
    let mut odd = vec![f32::NAN; 64 * 64];
    odd[16 * 64 + 10] = f32::INFINITY;
    odd[17 * 64 + 10] = -3.0;
    let mask = answer(odd).unwrap();
    assert_eq!(mask.iter().filter(|v| **v > 0).count(), 1);
    assert_eq!(mask[8 * 64 + 10], 255);
}

#[test]
fn bad_and_empty_pictures() {
    let ctx = RunContext::detached();
    let model = Red::new();
    // Bytes that do not match the stated size are refused, never indexed past.
    let short = matte(
        picture(&[1, 2, 3], 4, 4),
        MAX_RES,
        0.0,
        &mut |i| model.predict(i),
        &ctx,
    );
    assert_eq!(short.unwrap_err().kind, ErrorKind::InvalidRequest);
    let empty = matte(
        picture(&[], 0, 5),
        MAX_RES,
        0.0,
        &mut |i| model.predict(i),
        &ctx,
    )
    .unwrap();
    assert!(empty.is_empty());
    let one = run(picture(&[230, 50, 50], 1, 1), SHRINK, &model).unwrap();
    assert_eq!(one, [255]);
}
