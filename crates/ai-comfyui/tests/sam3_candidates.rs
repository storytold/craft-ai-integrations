//! Point-prompt segmentation candidates: every candidate the server returns, in server order,
//! with its mask, predicted score and measured bounds. Candidates are alternatives; nothing
//! here names them or picks one for the person.
mod sam3_support;
use craft_ai_comfyui::{Sam3Session, sam3};
use sam3_support::{Server, mask, pixels, server_nodes};
use serde_json::{Value, json};

const NODES: &[&str] = &[
    "PreviewAny",
    "StringConcatenate",
    "SAM3Segmentation",
    "SAM3PointCollector",
];

/// A PreviewAny report as the server returns it: the request's nonce, a newline, the JSON.
fn text(value: &str) -> Value {
    json!({"text": [format!("{{nonce}}\n{value}")]})
}

fn files(names: &[&str]) -> Value {
    Value::Array(
        names
            .iter()
            .map(|n| json!({"filename": n, "type": "output", "subfolder": ""}))
            .collect(),
    )
}

/// Three candidates in a 2x1 image. Scores are deliberately not in descending order.
fn candidate_server(scores: &str, nodes: &[&str]) -> Server {
    server_nodes(
        files(&["a.png", "b.png", "c.png"]),
        vec![
            ("a.png", mask(&[255, 0])),
            ("b.png", mask(&[255, 255])),
            ("c.png", mask(&[0, 255])),
        ],
        json!({"8": text(scores)}),
        nodes,
    )
}

fn session(server: &Server) -> Sam3Session {
    let mut session = Sam3Session::new(server.url.trim_end_matches("/v1")).unwrap();
    session.prepare(&mask(&[255, 255])).unwrap();
    session
}

fn prompts(server: &Server) -> Vec<Value> {
    server
        .recorded()
        .into_iter()
        .filter(|r| r.path == "/prompt")
        .map(|r| r.body_json()["prompt"].clone())
        .collect()
}

fn click() -> sam3::Prompt {
    sam3::Prompt::point(0.5, 0.5)
}

#[test]
fn candidates_keep_server_order_masks_scores_and_bounds_aligned() {
    let server = candidate_server("[0.2,0.9,0.5]", NODES);
    let found = session(&server).candidates(&click()).unwrap();
    assert_eq!(found.dimensions(), (2, 1));
    let list = found.candidates();
    assert_eq!(list.len(), 3);
    assert_eq!(
        list.iter().map(|c| c.index()).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(
        list.iter().map(|c| c.score()).collect::<Vec<_>>(),
        vec![0.2, 0.9, 0.5]
    );
    assert_eq!(pixels(list[0].mask_png()), vec![255, 0]);
    assert_eq!(pixels(list[1].mask_png()), vec![255, 255]);
    assert_eq!(pixels(list[2].mask_png()), vec![0, 255]);
    assert_eq!(list[0].mask_bounds(), Some([0, 0, 1, 1]));
    assert_eq!(list[1].mask_bounds(), Some([0, 0, 2, 1]));
    assert_eq!(list[2].mask_bounds(), Some([1, 0, 2, 1]));
    // The highest score is offered as a helper, never applied for the person.
    assert_eq!(found.highest_scoring(), Some(1));
}

/// Candidate requests send their points in pixels and ask for every candidate mask.
#[test]
fn the_graph_sends_points_in_pixels_and_asks_for_every_candidate() {
    let server = candidate_server("[0.2,0.9,0.5]", NODES);
    let prompt = sam3::Prompt {
        positive_points: vec![(0.5, 0.25), (1.5, 0.75)],
        negative_points: vec![(1.0, 0.5)],
        box_: None,
    };
    session(&server).candidates(&prompt).unwrap();
    let graph = prompts(&server).pop().unwrap();
    let parsed = |key: &str| -> Value {
        serde_json::from_str(graph["10"]["inputs"][key].as_str().unwrap()).unwrap()
    };
    assert_eq!(graph["10"]["class_type"], "SAM3PointCollector");
    assert_eq!(
        parsed("coordinates"),
        json!([{"x": 0.5, "y": 0.25}, {"x": 1.5, "y": 0.75}])
    );
    assert_eq!(parsed("neg_coordinates"), json!([{"x": 1.0, "y": 0.5}]));
    let seg = &graph["3"];
    assert_eq!(seg["class_type"], "SAM3Segmentation");
    assert_eq!(seg["inputs"]["positive_points"], json!(["10", 0]));
    assert_eq!(seg["inputs"]["negative_points"], json!(["10", 1]));
    assert_eq!(seg["inputs"]["use_multimask"], true);
    assert_eq!(seg["inputs"]["output_best_mask"], false);
    assert_eq!(seg["inputs"]["refinement_iterations"], 0);
    assert_eq!(graph["4"]["inputs"]["mask"], json!(["3", 0]));
    assert_eq!(graph["5"]["class_type"], "PreviewImage");
    assert_eq!(graph["6"]["inputs"]["string_b"], json!(["3", 4]));
    assert_eq!(graph["8"]["inputs"]["source"], json!(["6", 0]));
}

#[test]
fn a_repeat_changes_only_the_stamp_and_output_prefix() {
    let server = candidate_server("[0.2,0.9,0.5]", NODES);
    let session = session(&server);
    session.candidates(&click()).unwrap();
    session.candidates(&click()).unwrap();
    let graphs = prompts(&server);
    assert_eq!(graphs.len(), 3, "prepare plus two requests");
    for node in ["1", "2", "3", "4", "8", "10"] {
        assert_eq!(
            graphs[1][node], graphs[2][node],
            "node {node} stays cacheable"
        );
    }
    assert_ne!(
        graphs[1]["6"]["inputs"]["string_a"],
        graphs[2]["6"]["inputs"]["string_a"]
    );
    assert_ne!(
        graphs[1]["5"]["inputs"]["filename_prefix"],
        graphs[2]["5"]["inputs"]["filename_prefix"]
    );
}

#[test]
fn text_not_stamped_for_this_request_is_refused() {
    let stale = server_nodes(
        files(&["a.png"]),
        vec![("a.png", mask(&[255, 0]))],
        json!({"8": {"text": ["someone-else\n[0.5]"]}}),
        NODES,
    );
    let err = session(&stale).candidates(&click()).unwrap_err();
    assert!(err.message.contains("stale"), "{}", err.message);
    let bare = server_nodes(
        files(&["a.png"]),
        vec![("a.png", mask(&[255, 0]))],
        json!({"8": {"text": ["[0.5]"]}}),
        NODES,
    );
    assert!(session(&bare).candidates(&click()).is_err());
    let missing = server_nodes(
        files(&["a.png"]),
        vec![("a.png", mask(&[255, 0]))],
        json!({}),
        NODES,
    );
    assert!(session(&missing).candidates(&click()).is_err());
}

#[test]
fn a_score_count_that_differs_from_the_mask_count_fails_the_whole_result() {
    for scores in ["[0.2,0.9]", "[0.2,0.9,0.5,0.1]", "[]"] {
        let server = candidate_server(scores, NODES);
        assert!(
            session(&server).candidates(&click()).is_err(),
            "accepted scores {scores}"
        );
    }
    let err = session(&candidate_server("[0.2,0.9]", NODES))
        .candidates(&click())
        .unwrap_err();
    assert!(err.message.contains("do not match"), "{}", err.message);
}

#[test]
fn malformed_scores_and_oversized_text_are_refused() {
    for scores in [
        "not json",
        "{\"a\":1}",
        "[0.2,null,0.5]",
        "[0.2,\"x\",0.5]",
        "[0.2,0.9,true]",
    ] {
        let server = candidate_server(scores, NODES);
        assert!(
            session(&server).candidates(&click()).is_err(),
            "accepted scores {scores}"
        );
    }
    let huge = format!("[{}0.5]", "0.5,".repeat(40_000));
    let err = session(&candidate_server(&huge, NODES))
        .candidates(&click())
        .unwrap_err();
    assert!(err.message.contains("too large"), "{}", err.message);
}

#[test]
fn more_than_three_candidates_is_refused_before_any_download() {
    let names = ["a.png", "b.png", "c.png", "d.png"];
    let server = server_nodes(
        files(&names),
        names.iter().map(|n| (*n, mask(&[255, 0]))).collect(),
        json!({"8": text("[0.1,0.2,0.3,0.4]")}),
        NODES,
    );
    let err = session(&server).candidates(&click()).unwrap_err();
    assert!(err.message.contains("too many"), "{}", err.message);
    assert!(
        !server
            .recorded()
            .iter()
            .any(|r| r.path.starts_with("/view")),
        "no mask is downloaded for an oversized result"
    );
}

#[test]
fn an_empty_candidate_keeps_its_place_but_all_empty_is_an_error() {
    let mixed = server_nodes(
        files(&["a.png", "b.png"]),
        vec![("a.png", mask(&[0, 0])), ("b.png", mask(&[0, 255]))],
        json!({"8": text("[0.1,0.8]")}),
        NODES,
    );
    let found = session(&mixed).candidates(&click()).unwrap();
    assert_eq!(found.candidates()[0].mask_bounds(), None);
    assert_eq!(found.candidates()[1].mask_bounds(), Some([1, 0, 2, 1]));
    let empty = server_nodes(
        files(&["a.png", "b.png"]),
        vec![("a.png", mask(&[0, 0])), ("b.png", mask(&[0, 0]))],
        json!({"8": text("[0.1,0.8]")}),
        NODES,
    );
    let err = session(&empty).candidates(&click()).unwrap_err();
    assert!(err.message.contains("no object"), "{}", err.message);
}

#[test]
fn unsupported_or_invalid_prompts_never_reach_the_server_graph_queue() {
    let server = candidate_server("[0.2,0.9,0.5]", NODES);
    let session = session(&server);
    let too_many = sam3::Prompt {
        positive_points: (0..40).map(|_| (0.5, 0.5)).collect(),
        ..Default::default()
    };
    let cases = [
        ("empty", sam3::Prompt::default()),
        ("outside", sam3::Prompt::point(5.0, 0.5)),
        ("nan", sam3::Prompt::point(f32::NAN, 0.5)),
        ("too many points", too_many),
    ];
    for (name, prompt) in cases {
        assert!(session.candidates(&prompt).is_err(), "accepted {name}");
    }
    assert_eq!(
        prompts(&server).len(),
        1,
        "only the prepare graph was queued"
    );
}

#[test]
fn a_server_missing_a_required_node_is_refused_before_anything_is_queued() {
    for missing in NODES {
        let kept: Vec<&str> = NODES.iter().copied().filter(|n| n != missing).collect();
        let server = candidate_server("[0.2,0.9,0.5]", &kept);
        let err = session(&server).candidates(&click()).unwrap_err();
        assert!(
            err.message.contains("missing"),
            "{missing}: {}",
            err.message
        );
        assert_eq!(prompts(&server).len(), 1, "{missing}: only prepare queued");
    }
}

#[test]
fn candidates_need_a_prepared_image() {
    let session = Sam3Session::new("http://127.0.0.1:9").unwrap();
    assert!(session.candidates(&click()).is_err());
}
