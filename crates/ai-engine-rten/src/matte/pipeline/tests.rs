use super::*;

/// A fake BEN2 that answers with the picture's red channel (subject = red), and a fake ViTMatte
/// that answers with its trimap, counting the calls to each matting model.
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
        if large {
            self.large_calls += 1;
        } else {
            self.small_calls += 1;
        }
        let n = h * w;
        Ok(input[3 * n..4 * n]
            .iter()
            .map(|t| if *t >= 0.5 { 1.0 } else { 0.0 })
            .collect())
    }
}

/// A red disc on a grey backdrop.
fn disc(w: usize, h: usize) -> Rgb {
    let (cx, cy, r) = (w as f32 / 2.0, h as f32 / 2.0, h as f32 / 4.0);
    Rgb {
        w,
        h,
        px: (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 - cx, (i / w) as f32 - cy);
                if x * x + y * y < r * r {
                    [0.9, 0.1, 0.1]
                } else {
                    [0.3, 0.3, 0.3]
                }
            })
            .collect(),
    }
}

fn ctl() -> (impl Fn() -> Result<(), AiError>, impl Fn(f32)) {
    (|| Ok(()), |_| {})
}

#[test]
fn a_disc_is_cut_out_at_the_pictures_size() {
    let mut fake = Fake::default();
    let (check, progress) = ctl();
    let c = Ctl {
        check: &check,
        progress: &progress,
    };
    let a = cutout(&disc(2000, 1500), &mut fake, Quality::High, &c).unwrap();
    assert_eq!((a.w, a.h), (2000, 1500));
    assert!(a.v[750 * 2000 + 1000] > 0.95);
    assert!(a.v[20 * 2000 + 20] < 0.05);
    // The whole picture, then tiles along the contour.
    assert!(fake.subject_calls > 1);
    assert!(fake.small_calls >= 1);
    assert_eq!(fake.large_calls, 0, "High never runs the larger model");
}

#[test]
fn a_smooth_edge_never_runs_the_larger_model() {
    let mut fake = Fake::default();
    let (check, progress) = ctl();
    let c = Ctl {
        check: &check,
        progress: &progress,
    };
    cutout(&disc(1600, 1200), &mut fake, Quality::Max, &c).unwrap();
    assert_eq!(fake.large_calls, 0, "a disc has no strands");
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
    assert!(cutout(&disc(400, 300), &mut fake, Quality::High, &c).is_err());
    assert_eq!(fake.subject_calls, 0);
}

#[test]
fn a_tiny_picture_is_refused() {
    let mut fake = Fake::default();
    let (check, progress) = ctl();
    let c = Ctl {
        check: &check,
        progress: &progress,
    };
    let tiny = Rgb {
        w: 4,
        h: 4,
        px: vec![[0.0; 3]; 16],
    };
    assert!(cutout(&tiny, &mut fake, Quality::High, &c).is_err());
}

#[test]
fn a_model_answer_of_the_wrong_size_is_an_error_not_a_panic() {
    struct Short;
    impl Models for Short {
        fn subject(&mut self, _: Vec<f32>) -> Result<Vec<f32>, AiError> {
            Ok(vec![0.0; 10])
        }
        fn matte(&mut self, _: bool, _: Vec<f32>, _: usize, _: usize) -> Result<Vec<f32>, AiError> {
            Ok(vec![0.0; 10])
        }
    }
    let (check, progress) = ctl();
    let c = Ctl {
        check: &check,
        progress: &progress,
    };
    assert!(cutout(&disc(400, 300), &mut Short, Quality::High, &c).is_err());
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
fn strands_are_told_from_a_smooth_edge() {
    let t = Tile {
        x: 0,
        y: 0,
        w: 200,
        h: 200,
    };
    let coarse = Plane::from_fn(200, 200, |i| if i / 200 < 100 { 1.0 } else { 0.0 });
    assert!(!breaks_into_strands(&coarse, &coarse, t));
    let hair = Plane::from_fn(200, 200, |i| {
        let (x, y) = (i % 200, i / 200);
        if y < 100 || (y < 120 && x % 2 == 0) {
            1.0
        } else {
            0.0
        }
    });
    assert!(breaks_into_strands(&hair, &coarse, t));
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
