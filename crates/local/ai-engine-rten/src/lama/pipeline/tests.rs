use craft_ai_core::AiError;

use super::regions::{MAX_AREAS, areas};
use super::{Ctl, Inpainter, MAX_RIM, Rect, SIDE, remove, scaled_size};
use crate::picture::Rgb;

const GREEN: [f32; 3] = [20.0, 200.0, 60.0];
const PLANE: usize = SIDE * SIDE;

/// Keeps every model input, and answers green where the model's mask is set and zero elsewhere,
/// so a pixel written from outside the mask shows as black.
#[derive(Default)]
struct Recorder {
    images: Vec<Vec<f32>>,
    masks: Vec<Vec<f32>>,
}

impl Inpainter for Recorder {
    fn inpaint(&mut self, image: Vec<f32>, mask: Vec<f32>) -> Result<Vec<f32>, AiError> {
        assert_eq!((image.len(), mask.len()), (3 * PLANE, PLANE));
        assert!(image.iter().all(|v| (0.0..=1.0).contains(v)));
        assert!(mask.iter().all(|m| *m == 0.0 || *m == 1.0));
        let mut out = vec![0.0f32; 3 * PLANE];
        for (i, _) in mask.iter().enumerate().filter(|(_, m)| **m > 0.5) {
            for (c, value) in GREEN.iter().enumerate() {
                out[c * PLANE + i] = *value;
            }
        }
        self.images.push(image);
        self.masks.push(mask);
        Ok(out)
    }
}

fn flat(width: usize, height: usize, rgb: [u8; 3]) -> Rgb {
    Rgb {
        width,
        height,
        bytes: rgb.repeat(width * height),
    }
}

/// A mask with each `(x0, y0, x1, y1)` box marked.
fn marks(w: usize, h: usize, boxes: &[(usize, usize, usize, usize)]) -> Vec<u8> {
    let mut mask = vec![0u8; w * h];
    for &(x0, y0, x1, y1) in boxes {
        for y in y0..y1 {
            mask[y * w + x0..y * w + x1].fill(255);
        }
    }
    mask
}

fn run(picture: Rgb, mask: &[u8], model: &mut Recorder) -> Result<Vec<u8>, AiError> {
    let check = || Ok(());
    let progress = |_: u8| {};
    let ctl = Ctl {
        check: &check,
        progress: &progress,
    };
    remove(picture, mask, model, &ctl)
}

fn pixel(bytes: &[u8], w: usize, x: usize, y: usize) -> [u8; 3] {
    let at = (y * w + x) * 3;
    [bytes[at], bytes[at + 1], bytes[at + 2]]
}

/// The box around the set pixels in the first `rows` rows of a model mask.
fn marked_box(mask: &[f32], rows: usize) -> Option<(usize, usize)> {
    let set: Vec<(usize, usize)> = (0..rows)
        .flat_map(|y| (0..SIDE).map(move |x| (x, y)))
        .filter(|(x, y)| mask[y * SIDE + x] > 0.5)
        .collect();
    let xs = set.iter().map(|p| p.0);
    let ys = set.iter().map(|p| p.1);
    Some((
        xs.clone().max()? - xs.min()? + 1,
        ys.clone().max()? - ys.min()? + 1,
    ))
}

/// Two small marks in opposite corners of a large picture are two areas, each filled from its own
/// unscaled window, and nothing between them changes.
#[test]
fn far_apart_marks_are_filled_separately_at_full_size() {
    let (w, h) = (2048, 1536);
    let mask = marks(w, h, &[(100, 100, 140, 140), (1900, 1400, 1940, 1440)]);
    let found = areas(&mask, w, h);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!((found[0].x0, found[0].y0, found[0].x1), (100, 100, 140));
    assert_eq!((found[1].x0, found[1].y0, found[1].x1), (1900, 1400, 1940));

    let mut model = Recorder::default();
    let out = run(flat(w, h, [40, 60, 210]), &mask, &mut model).unwrap();
    assert_eq!(model.masks.len(), 2, "one model run for each area");
    // A 40 pixel square with its 2 pixel rim, pixel for pixel: the window was not scaled down.
    for seen in &model.masks {
        assert_eq!(seen.iter().filter(|m| **m > 0.5).count(), 1924);
    }
    assert_eq!(pixel(&out, w, 120, 120), [20, 200, 60]);
    assert_eq!(pixel(&out, w, 1920, 1420), [20, 200, 60]);
    for (x, y) in [(0, 0), (1000, 700), (2047, 1535), (97, 97), (143, 143)] {
        assert_eq!(pixel(&out, w, x, y), [40, 60, 210], "({x}, {y}) changed");
    }
}

/// Marks that one unscaled window holds are filled in one run.
#[test]
fn near_marks_share_one_run() {
    let (w, h) = (1000, 800);
    let mask = marks(w, h, &[(100, 100, 120, 120), (300, 100, 320, 120)]);
    assert_eq!(
        areas(&mask, w, h),
        [Rect {
            x0: 100,
            y0: 100,
            x1: 320,
            y1: 120
        }]
    );
    let mut model = Recorder::default();
    let out = run(flat(w, h, [200, 50, 50]), &mask, &mut model).unwrap();
    assert_eq!(model.masks.len(), 1);
    assert_eq!(pixel(&out, w, 110, 110), [20, 200, 60]);
    assert_eq!(pixel(&out, w, 310, 110), [20, 200, 60]);
    assert_eq!(pixel(&out, w, 210, 110), [200, 50, 50]);
}

/// Many scattered marks never ask for more than `MAX_AREAS` model runs, every mark is in an
/// area, and no area's rim reaches another's.
#[test]
fn scattered_marks_stay_within_the_run_limit() {
    let (w, h) = (4096, 4096);
    let dots: Vec<(usize, usize, usize, usize)> = (0..7)
        .flat_map(|j| (0..7).map(move |i| (i * 600 + 10, j * 600 + 10)))
        .map(|(x, y)| (x, y, x + 4, y + 4))
        .collect();
    let mask = marks(w, h, &dots);
    let found = areas(&mask, w, h);
    assert!((1..=MAX_AREAS).contains(&found.len()), "{}", found.len());
    for &(x0, y0, x1, y1) in &dots {
        let dot = Rect { x0, y0, x1, y1 };
        assert!(
            found.iter().any(|a| a.union(dot) == *a),
            "{dot:?} is in no area"
        );
    }
    for (i, a) in found.iter().enumerate() {
        for b in &found[i + 1..] {
            assert!(
                !a.grown(MAX_RIM, w, h).touches(b.grown(MAX_RIM, w, h)),
                "{a:?} reaches {b:?}"
            );
        }
    }
}

/// A window that is not square keeps its shape on the model's square, instead of being stretched
/// to fill it.
#[test]
fn a_wide_window_keeps_its_shape() {
    assert_eq!(scaled_size(64, 48), (64, 48));
    assert_eq!(scaled_size(512, 512), (512, 512));
    assert_eq!(scaled_size(1024, 512), (512, 256));
    assert_eq!(scaled_size(1200, 300), (512, 128));
    assert_eq!(scaled_size(2000, 1), (512, 1));

    let (w, h) = (1200, 300);
    let mask = marks(w, h, &[(150, 100, 1050, 200)]);
    let mut model = Recorder::default();
    let out = run(flat(w, h, [90, 90, 90]), &mask, &mut model).unwrap();
    assert_eq!(model.masks.len(), 1);
    // The marked strip with its rim is 924 by 124 pixels: about seven and a half times as wide
    // as it is tall, in the picture and on the model's square alike.
    let (wide, tall) = marked_box(&model.masks[0], 128).expect("marks in the scaled window");
    let shape = wide as f32 / tall as f32;
    assert!((6.5..8.5).contains(&shape), "{wide} by {tall}");
    assert_eq!(pixel(&out, w, 600, 150), [20, 200, 60]);
    assert_eq!(pixel(&out, w, 0, 0), [90, 90, 90]);
}

/// A window scaled down shows the model the average of the pixels each model pixel covers, not
/// one pixel picked from among them.
#[test]
fn a_scaled_window_averages_the_pixels_it_covers() {
    let (w, h) = (1024, 1024);
    let bytes: Vec<u8> = (0..w * h)
        .flat_map(|i| [if (i % w + i / w) % 2 == 0 { 0u8 } else { 255 }; 3])
        .collect();
    let picture = Rgb {
        width: w,
        height: h,
        bytes,
    };
    let mask = marks(w, h, &[(200, 200, 900, 900)]);
    let mut model = Recorder::default();
    run(picture, &mask, &mut model).unwrap();
    let (image, seen) = (&model.images[0], &model.masks[0]);
    let kept: Vec<f32> = (0..PLANE)
        .filter(|i| seen[*i] < 0.5)
        .map(|i| image[i])
        .collect();
    assert!(!kept.is_empty());
    assert!(
        kept.iter().all(|v| (v - 0.5).abs() < 0.01),
        "a checkerboard halved in size is mid grey"
    );
}

/// A small picture is padded by reflection: the model sees the mark mirrored, never an edge.
#[test]
fn a_small_picture_is_padded_by_reflection() {
    let (w, h) = (64, 48);
    let mask = marks(w, h, &[(24, 16, 40, 32)]);
    let mut model = Recorder::default();
    let out = run(flat(w, h, [220, 30, 40]), &mask, &mut model).unwrap();
    let seen = &model.masks[0];
    assert_eq!(seen[24 * SIDE + 32], 1.0, "the mark itself");
    assert_eq!(
        seen[24 * SIDE + (127 - 32)],
        1.0,
        "mirrored across the right"
    );
    assert_eq!(
        seen[(95 - 24) * SIDE + 32],
        1.0,
        "mirrored across the bottom"
    );
    assert_eq!(seen[0], 0.0);
    assert!((model.images[0][300 * SIDE + 300] - 220.0 / 255.0).abs() < 1e-6);
    assert_eq!(pixel(&out, w, 32, 24), [20, 200, 60]);
    // Just past the one pixel rim, inside the box around the rim: still the source pixel.
    assert_eq!(pixel(&out, w, 23, 15), [220, 30, 40]);
    assert_eq!(pixel(&out, w, 0, 0), [220, 30, 40]);
}

/// A mask that marks nothing has no areas and is refused before the model runs.
#[test]
fn a_blank_mask_has_no_areas() {
    let (w, h) = (300, 200);
    let mask = vec![0u8; w * h];
    assert!(areas(&mask, w, h).is_empty());
    let mut model = Recorder::default();
    let err = run(flat(w, h, [1, 2, 3]), &mask, &mut model).unwrap_err();
    assert!(err.message.contains("marks nothing"), "{}", err.message);
    assert!(model.masks.is_empty());
}

/// A mark inside the box around another, too far apart for one window, is filled with it: two
/// runs would each write into the other's pixels.
#[test]
fn areas_whose_rims_would_overlap_are_one() {
    let (w, h) = (1000, 1000);
    let corner = [(100, 100, 140, 900), (100, 860, 900, 900)];
    let dot = (600, 300, 620, 320);
    let mask = marks(w, h, &[corner[0], corner[1], dot]);
    assert_eq!(
        areas(&mask, w, h),
        [Rect {
            x0: 100,
            y0: 100,
            x1: 900,
            y1: 900
        }]
    );
}

/// A mark that belongs to another area but lies in this area's window is hidden from the model,
/// so the fill never copies from an object that is being removed, and is filled by its own run.
#[test]
fn another_areas_mark_is_hidden_from_the_model() {
    let (w, h) = (1000, 800);
    let mask = marks(w, h, &[(100, 100, 140, 140), (500, 100, 540, 140)]);
    assert_eq!(areas(&mask, w, h).len(), 2);
    let mut model = Recorder::default();
    let out = run(flat(w, h, [200, 50, 50]), &mask, &mut model).unwrap();
    assert_eq!(model.masks.len(), 2);
    // The first window is the picture's top-left 512 pixels, unscaled: it holds the left mark
    // and the near edge of the right one.
    let first = &model.masks[0];
    assert_eq!(first[120 * SIDE + 120], 1.0);
    assert_eq!(first[120 * SIDE + 505], 1.0, "the other area's mark");
    assert_eq!(first[120 * SIDE + 400], 0.0);
    assert_eq!(pixel(&out, w, 120, 120), [20, 200, 60]);
    assert_eq!(pixel(&out, w, 505, 120), [20, 200, 60]);
    assert_eq!(pixel(&out, w, 400, 120), [200, 50, 50]);
}
