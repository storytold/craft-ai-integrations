//! Opt-in real headless SAM 3 test. Use the audit runner to isolate the interactive node ID.
//! The runner forwards to the person's existing server and does not install, repair or start it.
use craft_ai_comfyui::{Sam3Session, sam3};

/// A mask PNG as (width, height, grey pixels).
fn decoded(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
        .read_info()
        .unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut pixels).unwrap();
    assert_eq!(frame.color_type, png::ColorType::Grayscale);
    pixels.truncate(frame.buffer_size());
    (frame.width, frame.height, pixels)
}

#[test]
#[ignore = "requires the existing headless server and public-domain astronaut fixture"]
fn installed_grounding_preserves_both_eyes_and_returns_their_exact_union() {
    let url = std::env::var("CRAFT_TEST_COMFYUI_URL").expect("set the isolated test URL");
    let image = std::env::var("CRAFT_TEST_SAM3_IMAGE").expect("set the public test image path");
    let input = std::fs::read(image).unwrap();
    let mut session = Sam3Session::new(&url).unwrap();
    session.verify_server().unwrap();
    session.prepare(&input).unwrap();
    let batch = session.ground_masks("eye").unwrap();
    assert_eq!(batch.dimensions(), (512, 512));
    assert_eq!(
        batch.masks().len(),
        2,
        "the NASA fixture has two distinct eyes"
    );
    let masks: Vec<_> = batch.masks().iter().map(|png| decoded(png).2).collect();
    assert!(masks.iter().all(|mask| mask.iter().any(|p| *p != 0)));
    assert!(
        masks[0]
            .iter()
            .zip(&masks[1])
            .all(|(a, b)| *a == 0 || *b == 0)
    );
    let expected: Vec<_> = masks[0]
        .iter()
        .zip(&masks[1])
        .map(|(a, b)| (*a).max(*b))
        .collect();
    assert_eq!(decoded(&batch.union_png().unwrap()).2, expected);
    // The existing API must also preserve all instances, not only the new batch API.
    let selected = session.ground("eye").unwrap();
    assert_eq!(decoded(&selected).2, expected);
    let foreground = expected.iter().filter(|p| **p != 0).count();
    println!(
        "LIVE SAM 3: {} instances, {}x{}, {} union pixels",
        batch.masks().len(),
        batch.dimensions().0,
        batch.dimensions().1,
        foreground
    );
}

#[test]
#[ignore = "requires the existing headless server and public-domain astronaut fixture"]
fn installed_grounding_reports_boxes_scores_and_picks_each_eye_on_a_repeat_too() {
    let url = std::env::var("CRAFT_TEST_COMFYUI_URL").expect("set the isolated test URL");
    let image = std::env::var("CRAFT_TEST_SAM3_IMAGE").expect("set the public test image path");
    let input = std::fs::read(image).unwrap();
    let mut session = Sam3Session::new(&url).unwrap();
    session.verify_server().unwrap();
    session.prepare(&input).unwrap();
    // The second identical request is fully cached on the server; it must still report everything.
    let mut summaries = Vec::new();
    for round in 0..2 {
        let found = session.ground_instances("eye").unwrap();
        assert_eq!(found.phrase(), "eye");
        assert_eq!(found.dimensions(), (512, 512));
        let list = found.instances();
        assert_eq!(list.len(), 2, "round {round}");
        assert!(
            list[0].score() >= list[1].score(),
            "server order is by score"
        );
        for instance in list {
            let [bx1, by1, bx2, by2] = instance.detector_box();
            let [mx1, my1, mx2, my2] = instance.mask_bounds().expect("eye mask has pixels");
            // The detector box and the measured mask extent agree to a few pixels.
            for (a, b) in [(bx1, mx1), (by1, my1), (bx2, mx2), (by2, my2)] {
                assert!((a - b as f32).abs() <= 6.0, "box {a} vs mask {b}");
            }
            let (cx, cy) = ((mx1 + mx2) / 2, (my1 + my2) / 2);
            let picked = found.pick(cx, cy);
            println!(
                "pick ({cx},{cy}) -> {picked:?} for instance {}",
                instance.index()
            );
        }
        // Each eye is found under its own centre; the corner of the image is nothing.
        let centre = |i: usize| {
            let [a, b, c, d] = list[i].mask_bounds().unwrap();
            ((a + c) / 2, (b + d) / 2)
        };
        let (x0, y0) = centre(0);
        let (x1, y1) = centre(1);
        assert!(found.pick(x0, y0).is_some() && found.pick(x1, y1).is_some());
        assert_eq!(found.pick(0, 0), None);
        let union = found.union_png().unwrap();
        assert!(!union.is_empty());
        summaries.push(
            list.iter()
                .map(|i| (i.score(), i.detector_box(), i.mask_bounds()))
                .collect::<Vec<_>>(),
        );
    }
    assert_eq!(
        summaries[0], summaries[1],
        "a cached repeat returns identical results"
    );
    println!("LIVE SAM 3 INSTANCES: {:?}", summaries[0]);
}

#[test]
#[ignore = "requires the existing headless server and public-domain astronaut fixture"]
fn installed_candidates_offer_alternatives_for_a_click_and_repeat_identically() {
    let url = std::env::var("CRAFT_TEST_COMFYUI_URL").expect("set the isolated test URL");
    let image = std::env::var("CRAFT_TEST_SAM3_IMAGE").expect("set the public test image path");
    let input = std::fs::read(image).unwrap();
    let mut session = Sam3Session::new(&url).unwrap();
    session.verify_server().unwrap();
    session.prepare(&input).unwrap();
    // A click on the astronaut's face: ambiguous between face, figure and a part of it.
    let click = sam3::Prompt::point(220.0, 110.0);
    let started = std::time::Instant::now();
    let first = session.candidates(&click).unwrap();
    let first_secs = started.elapsed().as_secs_f32();
    let started = std::time::Instant::now();
    let repeat = session.candidates(&click).unwrap();
    let repeat_secs = started.elapsed().as_secs_f32();
    assert_eq!(first.dimensions(), (512, 512));
    assert_eq!(
        first.candidates().len(),
        3,
        "multimask offers three alternatives"
    );
    let summary = |c: &craft_ai_comfyui::Sam3Candidates| {
        c.candidates()
            .iter()
            .map(|x| (x.score(), x.mask_bounds()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        summary(&first),
        summary(&repeat),
        "a cached repeat reports identically"
    );
    for candidate in first.candidates() {
        assert!(candidate.score().is_finite());
        assert!(
            candidate.mask_bounds().is_some(),
            "every candidate has pixels here"
        );
    }
    // At least one alternative covers the clicked pixel.
    let covers = |png: &[u8]| {
        let (w, _, pixels) = decoded(png);
        pixels[110 * w as usize + 220] >= 128
    };
    assert!(first.candidates().iter().any(|c| covers(c.mask_png())));
    // They really are alternatives, not three copies.
    let bounds: Vec<_> = first.candidates().iter().map(|c| c.mask_bounds()).collect();
    assert!(
        bounds[0] != bounds[1] || bounds[1] != bounds[2],
        "{bounds:?}"
    );
    // A negative point is accepted and changes what is offered.
    let refined = sam3::Prompt {
        positive_points: vec![(220.0, 110.0)],
        negative_points: vec![(100.0, 400.0)],
        box_: None,
    };
    let with_negative = session.candidates(&refined).unwrap();
    assert!(!with_negative.candidates().is_empty());
    println!(
        "LIVE SAM 3 CANDIDATES: first {first_secs:.2}s repeat {repeat_secs:.2}s highest {:?} {:?} negative {:?}",
        first.highest_scoring(),
        summary(&first),
        summary(&with_negative)
    );
}

#[test]
#[ignore = "requires the existing headless server and public-domain astronaut fixture"]
fn installed_box_prompt_follows_its_box_and_refinement_passes_run() {
    let url = std::env::var("CRAFT_TEST_COMFYUI_URL").expect("set the isolated test URL");
    let image = std::env::var("CRAFT_TEST_SAM3_IMAGE").expect("set the public test image path");
    let input = std::fs::read(image).unwrap();
    let mut session = Sam3Session::new(&url).unwrap();
    session.verify_server().unwrap();
    session.prepare(&input).unwrap();
    // A box around the astronaut's face (the face candidate measured [171, 75, 280, 191]).
    let around_face = sam3::Prompt {
        box_: Some([165.0, 70.0, 285.0, 195.0]),
        ..Default::default()
    };
    let started = std::time::Instant::now();
    let boxed = session.candidates(&around_face).unwrap();
    let box_secs = started.elapsed().as_secs_f32();
    assert!(!boxed.candidates().is_empty());
    // At least one alternative follows the box: its measured bounds sit inside it, close to it.
    let follows = boxed.candidates().iter().any(|c| {
        c.mask_bounds().is_some_and(|[x1, y1, x2, y2]| {
            x1 >= 160 && y1 >= 65 && x2 <= 290 && y2 <= 200 && (x2 - x1) >= 80 && (y2 - y1) >= 80
        })
    });
    assert!(follows, "{:?}", boxed.candidates());
    // A box over the empty background far from the figure must not behave like the face box.
    let elsewhere = sam3::Prompt {
        box_: Some([400.0, 400.0, 480.0, 480.0]),
        ..Default::default()
    };
    let other = session.candidates(&elsewhere);
    let summary = |c: &craft_ai_comfyui::Sam3Candidates| {
        c.candidates()
            .iter()
            .map(|x| (x.score(), x.mask_bounds()))
            .collect::<Vec<_>>()
    };
    if let Ok(other) = &other {
        assert_ne!(summary(&boxed), summary(other), "the box must matter");
    }
    // A box and a point together, and refinement passes on the same click.
    let both = sam3::Prompt {
        positive_points: vec![(220.0, 110.0)],
        box_: Some([165.0, 70.0, 285.0, 195.0]),
        ..Default::default()
    };
    let combined = session.candidates(&both).unwrap();
    let click = sam3::Prompt::point(220.0, 110.0);
    let plain = session.candidates(&click).unwrap();
    let started = std::time::Instant::now();
    let refined = session.candidates_refined(&click, 3).unwrap();
    let refine_secs = started.elapsed().as_secs_f32();
    assert_eq!(refined.candidates().len(), 3);
    assert!(refined.candidates().iter().all(|c| c.score().is_finite()));
    println!(
        "LIVE SAM 3 BOX/REFINE: box {box_secs:.2}s refine3 {refine_secs:.2}s\n box {:?}\n elsewhere {:?}\n box+point {:?}\n plain {:?}\n refined3 {:?}",
        summary(&boxed),
        other.as_ref().map(summary),
        summary(&combined),
        summary(&plain),
        summary(&refined)
    );
}
