use super::*;

/// A normalised click with the supplied position and inclusion sign.
fn click(x: f32, y: f32, positive: bool) -> Click {
    Click { x, y, positive }
}

/// Clicks come first in order, scaled to pixels of the 1024 square, labelled 1 on the object and
/// 0 off it.
#[test]
fn clicks_become_pixels_of_the_square_with_their_signs() {
    let (coords, labels) = prompt_points(&[click(0.5, 0.25, true), click(1.0, 0.0, false)], None);
    assert_eq!(coords, vec![512.0, 256.0, 1024.0, 0.0]);
    assert_eq!(labels, vec![1, 0]);
}

/// A box adds its top-left corner as label 2 and its bottom-right as label 3, after any clicks.
#[test]
fn a_box_adds_its_two_corners_after_the_clicks() {
    let area = Area {
        x0: 0.25,
        y0: 0.5,
        x1: 0.75,
        y1: 1.0,
    };
    let (coords, labels) = prompt_points(&[click(0.5, 0.5, true)], Some(area));
    assert_eq!(coords, vec![512.0, 512.0, 256.0, 512.0, 768.0, 1024.0]);
    assert_eq!(labels, vec![1, 2, 3]);
    let (coords, labels) = prompt_points(&[], Some(area));
    assert_eq!((coords.len(), labels), (4, vec![2, 3]));
}

/// The proposal with the highest score is chosen; scores that are not numbers never win, and the
/// first is the fallback when none is usable.
#[test]
fn the_highest_rated_proposal_wins_and_nonsense_scores_never_do() {
    assert_eq!(best_proposal(&[0.1, 0.9, 0.5, 0.2]), 1);
    assert_eq!(best_proposal(&[0.9, 0.1, 0.5, 0.95]), 3);
    assert_eq!(best_proposal(&[f32::NAN, 0.2, f32::INFINITY, 0.1]), 1);
    assert_eq!(best_proposal(&[f32::NAN, f32::NAN]), 0);
    assert_eq!(best_proposal(&[]), 0);
}
