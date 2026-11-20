use super::*;

#[test]
fn target_sizes() {
    assert_eq!(target_size(4000, 3000).unwrap(), (518, 392));
    assert_eq!(target_size(3000, 4000).unwrap(), (392, 518));
    assert_eq!(target_size(100, 100).unwrap(), (518, 518));
    assert_eq!(target_size(10_000, 10).unwrap(), (518, 14));
    assert_eq!(target_size(1, 1).unwrap(), (518, 518));
    for (w, h) in [(7, 5000), (4000, 3001), (640, 480), (1, 9999)] {
        let (a, b) = target_size(w, h).unwrap();
        assert_eq!(a % PATCH, 0);
        assert_eq!(b % PATCH, 0);
        assert_eq!(a.max(b), MODEL_SIZE);
        assert!(a >= PATCH && b >= PATCH);
    }
}

#[test]
fn a_picture_is_fitted_within_a_cap_keeping_its_shape() {
    assert_eq!(fit_within(4000, 3000, 2072), (2072, 1554));
    assert_eq!(fit_within(3000, 4000, 2072), (1554, 2072));
    assert_eq!(fit_within(1600, 1200, 2072), (1600, 1200));
    assert_eq!(fit_within(10_000, 1, 2072), (2072, 1));
}

#[test]
fn bad_input_is_rejected() {
    assert!(preprocess(&[], 0, 4).is_err());
    assert!(preprocess(&[0.0; 5], 2, 1).is_err());
    assert!(preprocess(&[f32::NAN; 6], 2, 1).is_err());
    assert!(target_size(u32::MAX, u32::MAX).is_err());
    assert!(postprocess(&[0.0; 3], 2, 2).is_err());
    assert!(postprocess(&[f32::NAN; 4], 2, 2).is_err());
}

#[test]
fn normalisation_values() {
    // A flat mid-grey image normalises to (0.5 - mean) / std in every plane.
    let p = preprocess(&vec![0.5; 8 * 6 * 3], 8, 6).unwrap();
    assert_eq!((p.width, p.height), (518, 392));
    let plane = (p.width * p.height) as usize;
    assert_eq!(p.data.len(), 3 * plane);
    for c in 0..3 {
        let want = (0.5 - MEAN[c]) / STD[c];
        assert!(
            p.data[c * plane..(c + 1) * plane]
                .iter()
                .all(|v| (v - want).abs() < 1e-4)
        );
    }
}

#[test]
fn downscale_averages() {
    // A black/white checkerboard shrinks to mid-grey (area average), not aliased.
    let (w, h) = (1036u32, 1036u32);
    let mut img = vec![0.0f32; (w * h * 3) as usize];
    for y in 0..h {
        for x in 0..w {
            if (x + y) % 2 == 0 {
                let i = ((y * w + x) * 3) as usize;
                img[i..i + 3].fill(1.0);
            }
        }
    }
    let p = preprocess(&img, w, h).unwrap();
    let want = (0.5 - MEAN[0]) / STD[0];
    let mid = p.data[(p.height as usize / 2) * p.width as usize + p.width as usize / 2];
    assert!((mid - want).abs() < 0.05, "{mid} vs {want}");
}

/// 8-bit pixels resize to 0..1 with the scale, and a slice that is too short does not panic.
#[test]
fn eight_bit_pixels_resize_to_unit_range() {
    let out = resize_rgb(&[255u8, 0, 51, 255, 0, 51], (2, 1), (1, 1), 1.0 / 255.0);
    assert_eq!(out.len(), 3);
    assert!((out[0] - 1.0).abs() < 1e-6 && out[1].abs() < 1e-6 && (out[2] - 0.2).abs() < 1e-6);
    let short = resize_rgb(&[255u8; 3], (2, 2), (2, 2), 1.0);
    assert_eq!(short.len(), 12);
}

#[test]
fn postprocess_normalises() {
    let m = postprocess(&[2.0, 4.0, 6.0, 10.0], 2, 2).unwrap();
    assert_eq!(m.data, vec![0.0, 0.25, 0.5, 1.0]);
    let flat = postprocess(&[3.0; 4], 2, 2).unwrap();
    assert!(flat.data.iter().all(|&v| v == 0.0));
}
