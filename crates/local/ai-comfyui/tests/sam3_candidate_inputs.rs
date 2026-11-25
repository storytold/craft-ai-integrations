//! Box prompts and refinement passes for point/box candidates. A box is sent as the
//! normalised centre and size the installed nodes expect; refinement is a bounded count.
mod sam3_support;
use craft_ai_comfyui::{Sam3Session, sam3};
use sam3_support::{Server, server_nodes, sized_mask};
use serde_json::{Value, json};

const NODES: &[&str] = &[
    "PreviewAny",
    "StringConcatenate",
    "SAM3Segmentation",
    "SAM3PointCollector",
    "SAM3CreateBox",
    "SAM3CombineBoxes",
];

fn text(value: &str) -> Value {
    json!({"text": [format!("{{nonce}}\n{value}")]})
}

/// One candidate in a 4x2 image.
fn box_server(nodes: &[&str]) -> Server {
    server_nodes(
        json!([{"filename": "a.png", "type": "output", "subfolder": ""}]),
        vec![("a.png", sized_mask(4, 2, &[255, 255, 0, 0, 255, 255, 0, 0]))],
        json!({"8": text("[0.7]")}),
        nodes,
    )
}

fn session(server: &Server) -> Sam3Session {
    let mut session = Sam3Session::new(server.url.trim_end_matches("/v1")).unwrap();
    session
        .prepare(&sized_mask(4, 2, &[255, 255, 255, 255, 255, 255, 255, 255]))
        .unwrap();
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

fn boxed(x1: f32, y1: f32, x2: f32, y2: f32) -> sam3::Prompt {
    sam3::Prompt {
        box_: Some([x1, y1, x2, y2]),
        ..Default::default()
    }
}

fn near(value: &Value, expected: f64) {
    let got = value.as_f64().unwrap_or(f64::NAN);
    assert!((got - expected).abs() < 1e-6, "{got} vs {expected}");
}

#[test]
fn a_box_is_sent_as_normalised_centre_and_size_through_the_box_nodes() {
    let server = box_server(NODES);
    let found = session(&server)
        .candidates(&boxed(1.0, 0.0, 3.0, 1.5))
        .unwrap();
    assert_eq!(found.candidates().len(), 1);
    let graph = prompts(&server).pop().unwrap();
    let create = &graph["11"];
    assert_eq!(create["class_type"], "SAM3CreateBox");
    near(&create["inputs"]["center_x"], 0.5);
    near(&create["inputs"]["center_y"], 0.375);
    near(&create["inputs"]["width"], 0.5);
    near(&create["inputs"]["height"], 0.75);
    assert_eq!(create["inputs"]["is_positive"], true);
    assert_eq!(graph["12"]["class_type"], "SAM3CombineBoxes");
    assert_eq!(graph["12"]["inputs"]["box_1"], json!(["11", 0]));
    assert_eq!(graph["3"]["inputs"]["box"], json!(["12", 0]));
    // A lone box has no points.
    assert_eq!(graph["10"]["inputs"]["coordinates"], "[]");
    assert_eq!(graph["10"]["inputs"]["neg_coordinates"], "[]");
}

#[test]
fn any_corner_order_and_fractions_normalise_the_same_way() {
    // Reversed in both axes, and reversed in only one axis either way round.
    for corners in [
        [3.5, 1.5, 0.5, 0.25],
        [3.5, 0.25, 0.5, 1.5],
        [0.5, 1.5, 3.5, 0.25],
        [0.5, 0.25, 3.5, 1.5],
    ] {
        let server = box_server(NODES);
        let [x1, y1, x2, y2] = corners;
        session(&server).candidates(&boxed(x1, y1, x2, y2)).unwrap();
        let create = &prompts(&server).pop().unwrap()["11"]["inputs"];
        near(&create["center_x"], 2.0 / 4.0);
        near(&create["center_y"], 0.875 / 2.0);
        near(&create["width"], 3.0 / 4.0);
        near(&create["height"], 1.25 / 2.0);
    }
}

#[test]
fn a_box_and_points_travel_together() {
    let server = box_server(NODES);
    let prompt = sam3::Prompt {
        positive_points: vec![(1.0, 1.0)],
        negative_points: vec![(3.0, 1.0)],
        box_: Some([0.0, 0.0, 2.0, 1.5]),
    };
    session(&server).candidates(&prompt).unwrap();
    let graph = prompts(&server).pop().unwrap();
    assert_eq!(graph["3"]["inputs"]["box"], json!(["12", 0]));
    assert_eq!(
        graph["10"]["inputs"]["coordinates"],
        "[{\"x\":1.0,\"y\":1.0}]"
    );
    assert_eq!(
        graph["10"]["inputs"]["neg_coordinates"],
        "[{\"x\":3.0,\"y\":1.0}]"
    );
}

#[test]
fn a_points_only_prompt_adds_no_box_nodes_and_needs_none_installed() {
    let without: Vec<&str> = NODES
        .iter()
        .copied()
        .filter(|n| !n.starts_with("SAM3CreateBox") && !n.starts_with("SAM3CombineBoxes"))
        .collect();
    let server = box_server(&without);
    session(&server)
        .candidates(&sam3::Prompt::point(1.0, 1.0))
        .unwrap();
    let graph = prompts(&server).pop().unwrap();
    assert!(graph.get("11").is_none() && graph.get("12").is_none());
    assert!(graph["3"]["inputs"].get("box").is_none());
}

#[test]
fn a_box_is_refused_before_anything_is_queued_when_its_nodes_are_missing() {
    for missing in ["SAM3CreateBox", "SAM3CombineBoxes"] {
        let kept: Vec<&str> = NODES.iter().copied().filter(|n| *n != missing).collect();
        let server = box_server(&kept);
        let err = session(&server)
            .candidates(&boxed(0.0, 0.0, 2.0, 1.5))
            .unwrap_err();
        assert!(
            err.message.contains("missing"),
            "{missing}: {}",
            err.message
        );
        assert_eq!(prompts(&server).len(), 1, "{missing}: only prepare queued");
    }
}

#[test]
fn invalid_boxes_never_reach_the_queue() {
    let server = box_server(NODES);
    let session = session(&server);
    let cases = [
        ("outside", boxed(0.0, 0.0, 5.0, 1.5)),
        ("on the far edge", boxed(0.0, 0.0, 4.0, 1.5)),
        ("negative", boxed(-1.0, 0.0, 2.0, 1.5)),
        ("degenerate width", boxed(1.0, 0.0, 1.5, 1.5)),
        ("degenerate height", boxed(0.0, 0.5, 3.0, 1.0)),
        ("nan", boxed(f32::NAN, 0.0, 2.0, 1.5)),
        ("infinite", boxed(0.0, 0.0, f32::INFINITY, 1.5)),
    ];
    for (name, prompt) in cases {
        assert!(session.candidates(&prompt).is_err(), "accepted {name}");
    }
    assert_eq!(prompts(&server).len(), 1, "only prepare queued");
}

#[test]
fn the_refinement_count_is_sent_and_bounded() {
    let server = box_server(NODES);
    let session = session(&server);
    let click = sam3::Prompt::point(1.0, 1.0);
    session.candidates(&click).unwrap();
    session.candidates_refined(&click, 3).unwrap();
    session.candidates_refined(&click, 10).unwrap();
    let graphs = prompts(&server);
    assert_eq!(graphs[1]["3"]["inputs"]["refinement_iterations"], 0);
    assert_eq!(graphs[2]["3"]["inputs"]["refinement_iterations"], 3);
    assert_eq!(graphs[3]["3"]["inputs"]["refinement_iterations"], 10);
    let err = session.candidates_refined(&click, 11).unwrap_err();
    assert!(err.message.contains("refinement"), "{}", err.message);
    assert!(session.candidates_refined(&click, u8::MAX).is_err());
    assert_eq!(prompts(&server).len(), 4, "refused counts queue nothing");
}

#[test]
fn candidates_without_a_prepared_image_still_fail_with_a_box_or_refinement() {
    let session = Sam3Session::new("http://127.0.0.1:9").unwrap();
    assert!(session.candidates(&boxed(0.0, 0.0, 2.0, 1.5)).is_err());
    assert!(
        session
            .candidates_refined(&sam3::Prompt::point(0.5, 0.5), 2)
            .is_err()
    );
}
