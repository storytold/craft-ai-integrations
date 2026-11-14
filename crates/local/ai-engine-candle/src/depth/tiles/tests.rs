use std::cell::Cell;

use super::*;

fn ramp(w: u32, h: u32) -> DepthMap {
    DepthMap {
        width: w,
        height: h,
        data: (0..w * h)
            .map(|i| (i % w) as f32 / (w - 1) as f32)
            .collect(),
    }
}

fn crop(m: &DepthMap, r: TileRect) -> DepthMap {
    let mut data = Vec::new();
    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            data.push(m.data[(y * m.width + x) as usize]);
        }
    }
    DepthMap {
        width: r.width(),
        height: r.height(),
        data,
    }
}

fn tile_count(w: u32, h: u32, max_tiles: u32) -> u32 {
    plan_tiles(w, h, max_tiles).len() as u32
}

/// Four overlapping tiles on a 2 x 2 grid, 25 % overlap.
fn four_tiles(w: u32, h: u32) -> Vec<TileRect> {
    let (tw, th) = (w * 5 / 8 + 1, h * 5 / 8 + 1);
    let (sx, sy) = (w - tw, h - th);
    let rect = |x0, y0, x1, y1| TileRect { x0, y0, x1, y1 };
    vec![
        rect(0, 0, tw, th),
        rect(sx, 0, w, th),
        rect(0, sy, tw, h),
        rect(sx, sy, w, h),
    ]
}

#[test]
fn plans_cover_the_image_within_the_cap() {
    for (w, h, cap) in [
        (100, 80, 16),
        (518, 518, 16),
        (1600, 1200, 16),
        (2072, 1554, 16),
        (2072, 1554, 4),
        (5000, 300, 16),
        (300, 5000, 6),
        (1, 1, 16),
        (2000, 2000, 1),
        (3000, 3000, 99),
    ] {
        let t = plan_tiles(w, h, cap);
        assert!(
            !t.is_empty() && t.len() as u32 <= cap.clamp(1, MAX_TILES),
            "{w}x{h} cap {cap}: {}",
            t.len()
        );
        let mut cov = vec![false; (w * h) as usize];
        for r in &t {
            assert!(r.x1 <= w && r.y1 <= h && r.width() > 0 && r.height() > 0);
            for y in r.y0..r.y1 {
                for x in r.x0..r.x1 {
                    cov[(y * w + x) as usize] = true;
                }
            }
        }
        assert!(cov.iter().all(|&c| c), "{w}x{h} cap {cap} not covered");
    }
    assert_eq!(tile_count(400, 300, 16), 1);
    assert!(tile_count(1600, 1200, 16) > 1);
}

#[test]
fn affine_tiles_merge_back_to_the_global_map() {
    let (w, h) = (120u32, 90u32);
    let global = ramp(w, h);
    // Each tile is a different affine transform of the global map.
    let tiles: Vec<(TileRect, DepthMap)> = four_tiles(w, h)
        .iter()
        .enumerate()
        .map(|(i, &r)| {
            let (a, b) = (0.5 + 0.4 * i as f32, 0.1 * i as f32 - 0.3);
            let mut m = crop(&global, r);
            m.data.iter_mut().for_each(|v| *v = (*v - b) / a);
            (r, m)
        })
        .collect();
    let out = merge_tiles(&global, &tiles).unwrap();
    for (o, g) in out.data.iter().zip(&global.data) {
        assert!((o - g).abs() < 1e-3, "{o} vs {g}");
    }
}

#[test]
fn overlaps_have_no_seams() {
    let (w, h) = (120u32, 90u32);
    let global = ramp(w, h);
    // Tiles disagree with each other: different affine maps plus a different mild curvature.
    let tiles: Vec<(TileRect, DepthMap)> = four_tiles(w, h)
        .iter()
        .enumerate()
        .map(|(i, &r)| {
            let mut m = crop(&global, r);
            let bend = 0.04 * (i as f32 - 1.5);
            m.data.iter_mut().for_each(|v| {
                *v = (*v + bend * *v * (1.0 - *v)) * (0.6 + 0.2 * i as f32) + 0.1 * i as f32
            });
            (r, m)
        })
        .collect();
    let out = merge_tiles(&global, &tiles).unwrap();
    let slope = 1.0 / (w - 1) as f32;
    for y in 0..h as usize {
        for x in 1..w as usize {
            let d = (out.data[y * w as usize + x] - out.data[y * w as usize + x - 1]).abs();
            assert!(d < slope * 3.0, "step {d} at ({x},{y})");
        }
    }
    for y in 1..h as usize {
        for x in 0..w as usize {
            let d = (out.data[y * w as usize + x] - out.data[(y - 1) * w as usize + x]).abs();
            assert!(d < slope * 1.5, "vertical step {d} at ({x},{y})");
        }
    }
}

#[test]
fn detail_only_in_tiles_survives() {
    let (w, h) = (120u32, 90u32);
    let global = ramp(w, h);
    let (bx, by) = (30usize, 20usize);
    let tiles: Vec<(TileRect, DepthMap)> = four_tiles(w, h)
        .iter()
        .map(|&r| {
            let mut m = crop(&global, r);
            // A narrow bump (detail the global pass lacks) in the first tile only.
            if r.x0 == 0 && r.y0 == 0 {
                m.data[by * m.width as usize + bx] += 0.2;
            }
            (r, m)
        })
        .collect();
    let out = merge_tiles(&global, &tiles).unwrap();
    let i = by * w as usize + bx;
    let around = (out.data[i - 1] + out.data[i + 1]) / 2.0;
    assert!(
        out.data[i] - around > 0.1,
        "bump lost: {} vs {around}",
        out.data[i]
    );
}

/// A tile with no structure (flat depth) is skipped: the pixels it covers keep the global map.
/// Before the fix such a tile was shifted to the global mean and blended in, which flattened the
/// whole region it covered and wiped the global ramp there.
#[test]
fn a_flat_tile_leaves_the_global_map_alone() {
    let global = ramp(40, 30);
    // Covers the left half, overlapping nothing else.
    let r = TileRect {
        x0: 0,
        y0: 0,
        x1: 20,
        y1: 30,
    };
    let flat = DepthMap {
        width: 20,
        height: 30,
        data: vec![0.7; 600],
    };
    let out = merge_tiles(&global, &[(r, flat)]).unwrap();
    assert_eq!(
        out.data, global.data,
        "the ramp must survive under a flat tile"
    );
    // Beside a tile that does have structure, the flat one adds nothing and takes nothing away.
    let right = TileRect {
        x0: 20,
        y0: 0,
        x1: 40,
        y1: 30,
    };
    let real = crop(&global, right);
    let both = merge_tiles(
        &global,
        &[
            (
                r,
                DepthMap {
                    width: 20,
                    height: 30,
                    data: vec![0.2; 600],
                },
            ),
            (right, real),
        ],
    )
    .unwrap();
    for (o, g) in both.data.iter().zip(&global.data) {
        assert!((o - g).abs() < 1e-3, "{o} vs {g}");
    }
}

#[test]
fn a_tile_without_weight_or_a_finite_fit_is_not_fitted() {
    assert_eq!(fit_affine(&[], &[], &[]), None);
    assert_eq!(fit_affine(&[1.0, 2.0], &[0.0, 1.0], &[0.0, 0.0]), None);
    assert_eq!(
        fit_affine(&[0.5; 4], &[0.1, 0.2, 0.3, 0.4], &[1.0; 4]),
        None
    );
    let (a, b) = fit_affine(&[0.0, 1.0], &[1.0, 3.0], &[1.0, 1.0]).unwrap();
    assert!((a - 2.0).abs() < 1e-5 && (b - 1.0).abs() < 1e-5, "{a} {b}");
}

#[test]
fn degenerate_and_invalid_input() {
    let global = ramp(40, 30);
    let r = TileRect {
        x0: 0,
        y0: 0,
        x1: 20,
        y1: 30,
    };
    // No tiles: the global map.
    assert_eq!(merge_tiles(&global, &[]).unwrap().data, global.data);
    // Mismatched or out-of-bounds tiles are errors.
    let bad = DepthMap {
        width: 3,
        height: 3,
        data: vec![0.0; 9],
    };
    assert!(merge_tiles(&global, &[(r, bad)]).is_err());
    let oob = TileRect {
        x0: 30,
        y0: 0,
        x1: 50,
        y1: 30,
    };
    assert!(
        merge_tiles(
            &global,
            &[(
                oob,
                DepthMap {
                    width: 20,
                    height: 30,
                    data: vec![0.0; 600]
                }
            )]
        )
        .is_err()
    );
    assert!(
        merge_tiles(
            &DepthMap {
                width: 0,
                height: 0,
                data: vec![]
            },
            &[]
        )
        .is_err()
    );
    let nan = DepthMap {
        width: 20,
        height: 30,
        data: vec![f32::NAN; 600],
    };
    assert!(merge_tiles(&global, &[(r, nan)]).is_err());
}

#[test]
fn resampling_converts_as_it_writes() {
    let up = resample_plane_with(&[0.0, 1.0], (2, 1), (4, 1), |v| (v * 100.0).round() as u16);
    assert_eq!(up.len(), 4);
    assert_eq!((up[0], up[3]), (0, 100));
    // An empty or short source reads as zeros at the asked size, not a panic.
    assert_eq!(
        resample_plane_with(&[], (0, 0), (2, 2), |v| v),
        vec![0.0; 4]
    );
    assert_eq!(
        resample_plane_with(&[1.0], (2, 2), (1, 2), |v| v),
        vec![0.0; 2]
    );
}

#[test]
fn tiled_pipeline_with_a_fake_estimator() {
    let (w, h) = (1100u32, 800u32);
    let rgb = vec![0.5f32; (w * h * 3) as usize];
    // The fake sees a different resolution per call: depth is the horizontal position.
    let est = |_: &[f32], tw: u32, th: u32| Ok(ramp(tw.min(64), th.min(48)));
    let calls = Cell::new(0u32);
    let last = Cell::new(-1.0f32);
    let m = estimate_tiled(&est, &rgb, (w, h), 16, &|f| {
        calls.set(calls.get() + 1);
        assert!(f >= last.get());
        last.set(f);
        Ok(())
    })
    .unwrap();
    assert_eq!((m.width, m.height), (w, h));
    assert_eq!(calls.get(), tile_count(w, h, 16) + 1);
    assert!(
        m.data
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    );
    // Cancel stops between tiles.
    let n = Cell::new(0);
    let r = estimate_tiled(&est, &rgb, (w, h), 16, &|_| {
        n.set(n.get() + 1);
        if n.get() > 2 {
            Err(AiError::cancelled())
        } else {
            Ok(())
        }
    });
    assert_eq!(r.err().unwrap().kind, craft_ai_core::ErrorKind::Cancelled);
    assert!(estimate_tiled(&est, &rgb[..10], (w, h), 16, &|_| Ok(())).is_err());
    let broken = |_: &[f32], _: u32, _: u32| {
        Ok(DepthMap {
            width: 4,
            height: 4,
            data: vec![0.5; 3],
        })
    };
    assert!(estimate_tiled(&broken, &rgb, (w, h), 16, &|_| Ok(())).is_err());
}
