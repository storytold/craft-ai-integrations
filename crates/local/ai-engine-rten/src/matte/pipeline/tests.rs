use super::*;

/// The small fake ViTMatte answers with its trimap (unknown as subject); the large one with
/// `LARGE` wherever the trimap is unknown. A matted edge pixel is then their mean, `(1 + LARGE)/2`.
const LARGE: f32 = 0.2;

/// A fake BEN2 that answers with the picture's red channel (subject = red), and the fake ViTMattes
/// above, counting the calls to each model.
#[derive(Default)]
struct Fake {
    subject_calls: usize,
    small_calls: usize,
    large_calls: usize,
}

impl Models for Fake {
    fn subject(&mut self, input: Vec<f32>) -> Result<Vec<f32>, AiError> {
        self.subject_calls += 1;
        let n = SIDE * SIDE;
        Ok((0..n)
            .map(|i| if input[i] > 0.5 { 1.0 } else { 0.0 })
            .collect())
    }

    fn matte(
        &mut self,
        large: bool,
        input: Vec<f32>,
        h: usize,
        w: usize,
    ) -> Result<Vec<f32>, AiError> {
        let n = h * w;
        let tri = &input[3 * n..4 * n];
        if large {
            self.large_calls += 1;
            Ok(tri
                .iter()
                .map(|t| if *t == 0.5 { LARGE } else { *t })
                .collect())
        } else {
            self.small_calls += 1;
            Ok(tri
                .iter()
                .map(|t| if *t >= 0.5 { 1.0 } else { 0.0 })
                .collect())
        }
    }
}

/// A `w` × `h` picture as 8-bit RGB: a red disc of radius `r` at (`cx`, `cy`) on grey.
fn disc_at(w: usize, h: usize, cx: f32, cy: f32, r: f32) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 3);
    for i in 0..w * h {
        let (x, y) = ((i % w) as f32 - cx, (i / w) as f32 - cy);
        out.extend_from_slice(if x * x + y * y < r * r {
            &[230, 26, 26]
        } else {
            &[77, 77, 77]
        });
    }
    out
}

/// A red disc in the middle of a grey backdrop.
fn disc(w: usize, h: usize) -> Vec<u8> {
    disc_at(w, h, w as f32 / 2.0, h as f32 / 2.0, h as f32 / 4.0)
}

fn ctl() -> (impl Fn() -> Result<(), AiError>, impl Fn(f32)) {
    (|| Ok(()), |_| {})
}

fn run(w: usize, h: usize, bytes: Vec<u8>, models: &mut dyn Models) -> Result<Plane, AiError> {
    let (check, progress) = ctl();
    let c = Ctl {
        check: &check,
        progress: &progress,
    };
    cutout(w, h, bytes, models, &c)
}

#[test]
fn a_disc_is_cut_out_at_the_pictures_size() {
    let mut fake = Fake::default();
    let a = run(2000, 1500, disc(2000, 1500), &mut fake).unwrap();
    assert_eq!((a.w, a.h), (2000, 1500));
    assert!(a.v[750 * 2000 + 1000] > 0.95);
    assert!(a.v[20 * 2000 + 20] < 0.05);
    // The whole picture, then tiles along the contour.
    assert!(fake.subject_calls > 1);
    assert!(fake.small_calls >= 1);
}

#[test]
fn every_edge_tile_is_matted_by_both_models_and_their_alphas_averaged() {
    let mut fake = Fake::default();
    let a = run(1600, 1200, disc(1600, 1200), &mut fake).unwrap();
    assert!(fake.small_calls >= 1);
    assert_eq!(fake.small_calls, fake.large_calls);
    // On the disc's rim (radius 300 around 800, 600), the trimap is unknown: the mean of the
    // small model's 1 and the large model's LARGE.
    let rim = a.v[600 * 1600 + 800 + 300];
    let mean = (1.0 + LARGE) / 2.0;
    assert!((rim - mean).abs() < 0.02, "rim {rim}, expected {mean}");
}

#[test]
fn an_edge_on_the_pictures_border_keeps_the_large_models_full_share() {
    // A disc cut by the left border, as hair is cut by the frame. Its edge meets the border at
    // y = 600 ± 300·sin(acos(0.6)) = 600 ± 240; pixels a few px from the border are in a tile
    // whose left edge is the picture's own.
    let (w, h) = (2400, 1200);
    let mut fake = Fake::default();
    let a = run(w, h, disc_at(w, h, 180.0, 600.0, 300.0), &mut fake).unwrap();
    let mean = (1.0 + LARGE) / 2.0;
    let mut seen = 0;
    for y in 300..900 {
        for x in 0..4 {
            let v = a.v[y * w + x];
            if v > 0.05 && v < 0.95 {
                assert!(
                    (v - mean).abs() < 0.02,
                    "at ({x}, {y}): {v}, expected {mean}"
                );
                seen += 1;
            }
        }
    }
    assert!(seen > 0, "no edge pixel next to the border");
}

#[test]
fn cancelling_stops_before_the_next_model_run() {
    let mut fake = Fake::default();
    let check = || Err(AiError::cancelled());
    let progress = |_| {};
    let c = Ctl {
        check: &check,
        progress: &progress,
    };
    assert!(cutout(400, 300, disc(400, 300), &mut fake, &c).is_err());
    assert_eq!(fake.subject_calls, 0);
}

#[test]
fn a_tiny_or_short_picture_is_refused() {
    let mut fake = Fake::default();
    assert!(run(4, 4, vec![0; 48], &mut fake).is_err());
    assert!(run(400, 300, vec![0; 10], &mut fake).is_err());
    assert_eq!(fake.subject_calls, 0);
}

/// Answers of ten values whatever the size asked.
struct Short;

impl Models for Short {
    fn subject(&mut self, _: Vec<f32>) -> Result<Vec<f32>, AiError> {
        Ok(vec![0.0; 10])
    }
    fn matte(&mut self, _: bool, _: Vec<f32>, _: usize, _: usize) -> Result<Vec<f32>, AiError> {
        Ok(vec![0.0; 10])
    }
}

#[test]
fn a_subject_answer_of_the_wrong_size_is_an_error_not_a_panic() {
    assert!(run(400, 300, disc(400, 300), &mut Short).is_err());
}

#[test]
fn a_matting_answer_of_the_wrong_size_is_an_error_not_a_panic() {
    let img = Rgb {
        w: 64,
        h: 64,
        px: vec![[0.5; 3]; 64 * 64],
    };
    let tri = vec![UNKNOWN; 64 * 64];
    let t = Tile {
        x: 0,
        y: 0,
        w: 64,
        h: 64,
    };
    for large in [false, true] {
        assert!(vitmatte(&mut Short, large, &img, &tri, t).is_err());
    }
}

#[test]
fn subject_answers_are_read_as_probabilities() {
    // All zero is backdrop, not a logit of 50%; values past the ends are clamped; NaN is backdrop.
    assert_eq!(probabilities(vec![0.0, 0.0]), vec![0.0, 0.0]);
    assert_eq!(
        probabilities(vec![-0.5, 0.25, 1.5, f32::NAN]),
        vec![0.0, 0.25, 1.0, 0.0]
    );
}

#[test]
fn the_close_look_removes_freely_but_adds_only_where_the_whole_look_saw_something() {
    // Three 20-px stripes: subject the close look removes, backdrop it would add, and a part the
    // whole look saw at 80% that it confirms.
    let stripe = |i: usize| (i % 60) / 20;
    let g = Plane::from_fn(60, 20, |i| [1.0, 0.0, 0.8][stripe(i)]);
    let acc = Plane::from_fn(60, 20, |i| [0.0, 1.0, 1.0][stripe(i)]);
    let ws = Plane::new(60, 20, 1.0);
    let band = Plane::new(60, 20, 1.0);
    let c = combine_close_look(&g, &acc, &ws, &band);
    let at = |x: usize| c.v[10 * 60 + x];
    assert!(at(10) < 0.01, "removed: {}", at(10));
    assert!(
        at(30) < 0.01,
        "not added where the whole look saw nothing: {}",
        at(30)
    );
    assert!(at(50) > 0.95, "added where it saw the subject: {}", at(50));
}

#[test]
fn the_trimap_is_sure_only_away_from_the_edge() {
    // Left half subject, right half backdrop, a soft column between.
    let coarse = Plane::from_fn(40, 10, |i| match i % 40 {
        0..19 => 1.0,
        19 | 20 => 0.5,
        _ => 0.0,
    });
    let tri = trimap(&coarse, 3);
    let at = |x: usize| tri[5 * 40 + x];
    assert_eq!(at(0), SUBJECT);
    assert_eq!(at(39), BACKDROP);
    for x in 16..24 {
        assert_eq!(at(x), UNKNOWN, "x = {x}");
    }
}

#[test]
fn fit_shrinks_to_the_side_and_keeps_colour() {
    let img = Rgb {
        w: 600,
        h: 300,
        px: vec![[0.2, 0.4, 0.6]; 600 * 300],
    };
    let f = img.fit(200);
    assert_eq!((f.w, f.h), (200, 100));
    assert!(f.px.iter().all(|p| (p[1] - 0.4).abs() < 1e-4));
    assert_eq!(img.fit(1000).w, 600);
}

#[test]
fn fitting_bytes_matches_fitting_floats() {
    let bytes: Vec<u8> = (0..600 * 300 * 3).map(|i| (i * 7 % 251) as u8).collect();
    let floats = Rgb {
        w: 600,
        h: 300,
        px: bytes
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| p.map(|v| f32::from(v) / 255.0))
            .collect(),
    };
    for side in [200, 1000] {
        let (a, b) = (
            Rgb::fit_bytes(600, 300, bytes.clone(), side),
            floats.fit(side),
        );
        assert_eq!((a.w, a.h), (b.w, b.h));
        assert!(
            a.px.iter()
                .zip(&b.px)
                .all(|(p, q)| (0..3).all(|c| (p[c] - q[c]).abs() < 1e-5))
        );
    }
}
