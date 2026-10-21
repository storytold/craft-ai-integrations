//! The segment contract: what is accepted, what is refused, and what survives a round trip.
use super::*;
use crate::capability::CapabilityId;
use crate::model::InputRole;

/// A bare `image.segment` request.
fn request() -> Request {
    Request::new(CapabilityId::parse(CAPABILITY).unwrap())
}

/// Clicks and a box written by the helpers read back unchanged, and a request with neither has
/// no clicks and no box.
#[test]
fn clicks_and_a_box_survive_a_round_trip() {
    let written = [
        Click {
            x: 0.25,
            y: 0.5,
            positive: true,
        },
        Click {
            x: 1.0,
            y: 0.0,
            positive: false,
        },
    ];
    let boxed = Area {
        x0: 0.1,
        y0: 0.2,
        x1: 0.6,
        y1: 0.9,
    };
    let r = with_area(with_clicks(request(), &written), boxed);
    assert_eq!(clicks(&r).unwrap(), written);
    assert_eq!(area(&r).unwrap(), Some(boxed));
    assert!(clicks(&request()).unwrap().is_empty());
    assert_eq!(area(&request()).unwrap(), None);
}

/// A click outside the picture, a missing field, a non-list or too many clicks is refused.
#[test]
fn clicks_outside_the_picture_or_too_many_are_refused() {
    for bad in [
        json!([{"x": 1.5, "y": 0.5, "positive": true}]),
        json!([{"x": -0.1, "y": 0.5, "positive": true}]),
        json!([{"x": 0.5, "y": 0.5}]),
        json!([{"x": "0.5", "y": 0.5, "positive": true}]),
        json!({"x": 0.5, "y": 0.5, "positive": true}),
        json!([[0.5, 0.5, true]]),
    ] {
        assert!(
            clicks(&request().with_param("points", bad.clone())).is_err(),
            "{bad}"
        );
    }
    let many: Vec<Click> = (0..=MAX_POINTS)
        .map(|_| Click {
            x: 0.5,
            y: 0.5,
            positive: true,
        })
        .collect();
    assert!(clicks(&with_clicks(request(), &many)).is_err());
    assert!(clicks(&with_clicks(request(), &many[..MAX_POINTS])).is_ok());
}

/// A box that is turned round, flat, or reaches outside the picture is refused.
#[test]
fn a_box_turned_round_flat_or_outside_is_refused() {
    for bad in [
        Area {
            x0: 0.6,
            y0: 0.2,
            x1: 0.1,
            y1: 0.9,
        },
        Area {
            x0: 0.1,
            y0: 0.9,
            x1: 0.6,
            y1: 0.2,
        },
        Area {
            x0: 0.1,
            y0: 0.2,
            x1: 0.1,
            y1: 0.9,
        },
        Area {
            x0: 0.1,
            y0: 0.2,
            x1: 1.2,
            y1: 0.9,
        },
    ] {
        assert!(area(&with_area(request(), bad)).is_err(), "{bad:?}");
    }
    assert!(area(&request().with_param("box", json!([0.1, 0.2, 0.6, 0.9]))).is_err());
}

/// The description is the prompt's text, and a blank prompt counts as none.
#[test]
fn a_blank_prompt_is_no_description() {
    let asked = request().with_input(InputRole::Prompt, Payload::text("the dog"));
    assert_eq!(description(&asked), Some("the dog"));
    let blank = request().with_input(InputRole::Prompt, Payload::text("  "));
    assert_eq!(description(&blank), None);
    assert_eq!(description(&request()), None);
}

/// A result written with [`found_output`] reads back, the mask is found beside it, and a result
/// with no [`Found`] output reads as none.
#[test]
fn the_result_and_its_mask_read_back() {
    let found = Found {
        matched: true,
        score: Some(0.9),
        name: Some("dog".into()),
    };
    let mask_out = Output {
        payload: Payload::inline("image/png", vec![1, 2, 3]).unwrap(),
        label: None,
    };
    let outputs = vec![mask_out.clone(), found_output(&found).unwrap()];
    assert_eq!(read_found(&outputs), Some(found));
    assert_eq!(mask(&outputs), Some(&mask_out.payload));
    assert_eq!(read_found(&outputs[..1]), None);
    assert_eq!(mask(&outputs[1..]), None);
}

/// A JPEG preview or an image handle that comes before the mask is not taken for it, and an
/// answer with only such images has no mask.
#[test]
fn only_an_inline_png_is_the_mask() {
    let output = |payload: Payload| Output {
        payload,
        label: None,
    };
    let jpeg = output(Payload::inline("image/jpeg", vec![9, 9]).unwrap());
    let handle = output(Payload::Handle(crate::payload::PayloadHandle {
        uri: "file:///elsewhere.png".into(),
        media_type: crate::payload::MediaType::new("image/png").unwrap(),
        size_bytes: None,
    }));
    let png = output(Payload::inline("image/png", vec![1, 2, 3]).unwrap());
    let outputs = vec![jpeg.clone(), handle.clone(), png.clone()];
    assert_eq!(mask(&outputs), Some(&png.payload));
    assert_eq!(mask(&[jpeg, handle]), None);
}

/// A name that is too long, blank or holds control characters, and a score outside 0 to 1, are
/// dropped on reading, so another program cannot put them on the screen. The match stays.
#[test]
fn a_hostile_name_or_score_is_dropped_but_the_match_stays() {
    let long = "x".repeat(MAX_NAME_CHARS + 1);
    for (name, score) in [(long.as_str(), 1.5), ("dog\u{7}", -0.1), ("   ", f32::NAN)] {
        // `json!` writes NaN as null, which reads as no score.
        let data =
            serde_json::to_vec(&json!({"matched": true, "name": name, "score": score})).unwrap();
        let out = Output {
            payload: Payload::inline(FOUND_MEDIA_TYPE, data).unwrap(),
            label: None,
        };
        let read = read_found(&[out]).unwrap();
        assert!(read.matched);
        assert_eq!((read.name, read.score), (None, None), "{name:?} {score}");
    }
    let fine = "x".repeat(MAX_NAME_CHARS);
    let found = Found {
        matched: true,
        score: Some(0.0),
        name: Some(fine.clone()),
    };
    let read = read_found(&[found_output(&found).unwrap()]).unwrap();
    assert_eq!(read.name, Some(fine));
}
