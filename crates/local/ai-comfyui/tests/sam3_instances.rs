//! Typed per-instance grounding results: mask, detector box, score and mask-measured bounds
//! stay aligned, and anything inconsistent fails the whole result.
mod sam3_support;
use craft_ai_comfyui::Sam3Session;
use sam3_support::{Server, mask, pixels, server_with, two_masks};
use serde_json::{Value, json};

/// A PreviewAny report as the server returns it: the request's nonce, a newline, the JSON.
fn text(value: &str) -> Value {
    json!({"text": [format!("{{nonce}}\n{value}")]})
}

/// Two instances in a 2x1 image: left pixel (score 0.9) and right pixel (score 0.5).
fn detail_server(boxes: &str, scores: &str) -> Server {
    server_with(
        two_masks(),
        vec![
            ("left.png", mask(&[255, 0])),
            ("right.png", mask(&[0, 255])),
        ],
        json!({"8": text(boxes), "9": text(scores)}),
        true,
    )
}

fn last_prompt(server: &Server) -> Value {
    server
        .recorded()
        .into_iter()
        .rfind(|r| r.path == "/prompt")
        .unwrap()
        .body_json()["prompt"]
        .clone()
}

fn session(server: &Server) -> Sam3Session {
    let mut session = Sam3Session::new(server.url.trim_end_matches("/v1")).unwrap();
    session.prepare(&mask(&[255, 255])).unwrap();
    session
}

const BOXES: &str = "[[0.0,0.0,1.0,1.0],[1.0,0.0,2.0,1.0]]";
const SCORES: &str = "[0.9,0.5]";

#[test]
fn instances_keep_mask_box_score_and_measured_bounds_aligned() {
    let server = detail_server(BOXES, SCORES);
    let found = session(&server).ground_instances("eye").unwrap();
    assert_eq!(found.phrase(), "eye");
    assert_eq!(found.dimensions(), (2, 1));
    let list = found.instances();
    assert_eq!(list.len(), 2);
    assert_eq!(pixels(list[0].mask_png()), vec![255, 0]);
    assert_eq!(pixels(list[1].mask_png()), vec![0, 255]);
    assert_eq!((list[0].index(), list[1].index()), (0, 1));
    assert_eq!((list[0].score(), list[1].score()), (0.9, 0.5));
    assert_eq!(list[0].detector_box(), [0.0, 0.0, 1.0, 1.0]);
    assert_eq!(list[1].detector_box(), [1.0, 0.0, 2.0, 1.0]);
    assert_eq!(list[0].mask_bounds(), Some([0, 0, 1, 1]));
    assert_eq!(list[1].mask_bounds(), Some([1, 0, 2, 1]));
    assert_eq!(pixels(&found.union_png().unwrap()), vec![255, 255]);
}

#[test]
fn the_grounding_graph_reads_boxes_and_scores_through_a_request_stamped_preview() {
    let server = detail_server(BOXES, SCORES);
    session(&server).ground_instances("eye").unwrap();
    let prompt = last_prompt(&server);
    let nonce = prompt["6"]["inputs"]["string_a"].as_str().unwrap();
    assert!(
        nonce.len() >= 8
            && nonce
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    );
    assert_eq!(prompt["6"]["class_type"], "StringConcatenate");
    assert_eq!(prompt["6"]["inputs"]["string_b"], json!(["3", 2]));
    assert_eq!(prompt["6"]["inputs"]["delimiter"], "\n");
    assert_eq!(prompt["7"]["class_type"], "StringConcatenate");
    assert_eq!(prompt["7"]["inputs"]["string_a"], nonce);
    assert_eq!(prompt["7"]["inputs"]["string_b"], json!(["3", 3]));
    assert_eq!(prompt["8"]["class_type"], "PreviewAny");
    assert_eq!(prompt["8"]["inputs"]["source"], json!(["6", 0]));
    assert_eq!(prompt["9"]["class_type"], "PreviewAny");
    assert_eq!(prompt["9"]["inputs"]["source"], json!(["7", 0]));
}

#[test]
fn a_repeat_changes_only_the_stamp_so_a_cached_run_still_reports() {
    let server = detail_server(BOXES, SCORES);
    let session = session(&server);
    session.ground_instances("eye").unwrap();
    session.ground_instances("eye").unwrap();
    let graphs: Vec<Value> = server
        .recorded()
        .into_iter()
        .filter(|r| r.path == "/prompt")
        .map(|r| r.body_json()["prompt"].clone())
        .collect();
    assert_eq!(graphs.len(), 3, "prepare plus two groundings");
    for node in ["1", "2", "3", "4", "8", "9"] {
        assert_eq!(
            graphs[1][node], graphs[2][node],
            "node {node} must stay cacheable"
        );
    }
    assert_ne!(
        graphs[1]["6"]["inputs"]["string_a"],
        graphs[2]["6"]["inputs"]["string_a"]
    );
    assert_ne!(
        graphs[1]["7"]["inputs"]["string_a"],
        graphs[2]["7"]["inputs"]["string_a"]
    );
    assert_ne!(
        graphs[1]["5"]["inputs"]["filename_prefix"],
        graphs[2]["5"]["inputs"]["filename_prefix"]
    );
}

#[test]
fn text_stamped_for_another_request_is_refused_as_stale() {
    let server = server_with(
        two_masks(),
        vec![
            ("left.png", mask(&[255, 0])),
            ("right.png", mask(&[0, 255])),
        ],
        json!({
            "8": {"text": [format!("someone-else\n{BOXES}")]},
            "9": text(SCORES),
        }),
        true,
    );
    let err = session(&server).ground_instances("eye").unwrap_err();
    assert!(err.message.contains("stale"), "{}", err.message);
    let unstamped = server_with(
        two_masks(),
        vec![
            ("left.png", mask(&[255, 0])),
            ("right.png", mask(&[0, 255])),
        ],
        json!({"8": {"text": [BOXES]}, "9": text(SCORES)}),
        true,
    );
    assert!(session(&unstamped).ground_instances("eye").is_err());
}

#[test]
fn a_box_or_score_count_that_differs_from_the_masks_fails_the_whole_result() {
    for (boxes, scores) in [
        ("[[0.0,0.0,1.0,1.0]]", SCORES),
        (BOXES, "[0.9]"),
        (BOXES, "[0.9,0.5,0.4]"),
    ] {
        let server = detail_server(boxes, scores);
        let err = session(&server).ground_instances("eye").unwrap_err();
        assert!(err.message.contains("do not match"), "{}", err.message);
    }
}

#[test]
fn consistent_boxes_and_scores_for_a_different_number_of_masks_fail_the_whole_result() {
    // Two boxes and two scores, but the server saved only one mask.
    let one_mask = server_with(
        json!([{"filename":"left.png","type":"output","subfolder":""}]),
        vec![("left.png", mask(&[255, 0]))],
        json!({"8": text(BOXES), "9": text(SCORES)}),
        true,
    );
    let err = session(&one_mask).ground_instances("eye").unwrap_err();
    assert!(err.message.contains("do not match"), "{}", err.message);
    // One box and one score, but two masks.
    let extra_mask = detail_server("[[0.0,0.0,1.0,1.0]]", "[0.9]");
    let err = session(&extra_mask).ground_instances("eye").unwrap_err();
    assert!(err.message.contains("do not match"), "{}", err.message);
}

#[test]
fn malformed_or_impossible_detector_metadata_is_refused() {
    let cases = [
        ("not json", SCORES),
        (BOXES, "not json"),
        ("[[0.0,0.0,1.0]]", SCORES),
        ("[[0.0,0.0,1.0,1.0],[1.0,0.0,3.0,1.0]]", SCORES),
        ("[[0.0,0.0,1.0,1.0],[1.5,0.0,1.0,1.0]]", SCORES),
        ("[[0.0,0.0,1.0,1.0],[-1.0,0.0,1.0,1.0]]", SCORES),
        (BOXES, "[0.9,1.5]"),
        (BOXES, "[0.9,-0.1]"),
        (BOXES, "[0.9,null]"),
        ("[[0.0,0.0,1.0,1.0],[1.0,0.0,2.0,\"x\"]]", SCORES),
    ];
    for (boxes, scores) in cases {
        let server = detail_server(boxes, scores);
        assert!(
            session(&server).ground_instances("eye").is_err(),
            "accepted boxes {boxes} scores {scores}"
        );
    }
}

#[test]
fn oversized_metadata_text_is_refused() {
    let huge = format!("[{}]", "0.5,".repeat(40_000) + "0.5");
    let server = detail_server(BOXES, &huge);
    let err = session(&server).ground_instances("eye").unwrap_err();
    assert!(err.message.contains("too large"), "{}", err.message);
}

#[test]
fn zero_detections_is_a_no_match_error_not_an_empty_result() {
    let server = server_with(
        json!([{"filename":"left.png","type":"output","subfolder":""}]),
        vec![("left.png", mask(&[0, 0]))],
        json!({"8": text("[]"), "9": text("[]")}),
        true,
    );
    let err = session(&server).ground_instances("unicorn").unwrap_err();
    assert!(
        err.message.starts_with("Nothing matched"),
        "{}",
        err.message
    );
}

#[test]
fn a_server_without_preview_any_is_refused_before_anything_is_queued() {
    let server = server_with(
        two_masks(),
        vec![
            ("left.png", mask(&[255, 0])),
            ("right.png", mask(&[0, 255])),
        ],
        json!({}),
        false,
    );
    let err = session(&server).ground_instances("eye").unwrap_err();
    assert!(err.message.contains("StringConcatenate"), "{}", err.message);
    // Only the prepare graph was queued.
    let queued = server
        .recorded()
        .iter()
        .filter(|r| r.path == "/prompt")
        .count();
    assert_eq!(queued, 1);
}

#[test]
fn a_missing_detail_output_is_an_error_not_an_unscored_result() {
    let server = server_with(
        two_masks(),
        vec![
            ("left.png", mask(&[255, 0])),
            ("right.png", mask(&[0, 255])),
        ],
        json!({"8": text(BOXES)}),
        true,
    );
    assert!(session(&server).ground_instances("eye").is_err());
}

#[test]
fn picking_chooses_the_highest_scoring_instance_covering_the_pixel() {
    // Instance 0 (score 0.4) covers both pixels; instance 1 (score 0.8) covers only the first.
    let server = server_with(
        two_masks(),
        vec![
            ("left.png", mask(&[255, 255])),
            ("right.png", mask(&[255, 0])),
        ],
        json!({
            "8": text("[[0.0,0.0,2.0,1.0],[0.0,0.0,1.0,1.0]]"),
            "9": text("[0.4,0.8]"),
        }),
        true,
    );
    let found = session(&server).ground_instances("eye").unwrap();
    assert_eq!(found.pick(0, 0), Some(1));
    assert_eq!(found.pick(1, 0), Some(0));
    assert_eq!(found.pick(2, 0), None);
    assert_eq!(found.pick(0, 1), None);
}

#[test]
fn picking_ignores_pixels_below_half_coverage() {
    let server = server_with(
        json!([{"filename":"left.png","type":"output","subfolder":""}]),
        vec![("left.png", mask(&[100, 200]))],
        json!({"8": text("[[0.0,0.0,2.0,1.0]]"), "9": text("[0.7]")}),
        true,
    );
    let found = session(&server).ground_instances("eye").unwrap();
    assert_eq!(found.pick(0, 0), None);
    assert_eq!(found.pick(1, 0), Some(0));
    assert_eq!(found.instances()[0].mask_bounds(), Some([0, 0, 2, 1]));
}
