//! Bounded-mask regression cases. Fixtures exercise the real client over scripted HTTP.
mod sam3_support;
use craft_ai_comfyui::Sam3Session;
use sam3_support::{mask, pixels, server, sized_mask, two_masks};
use serde_json::{Value, json};

fn prepared(server: &sam3_support::Server) -> Sam3Session {
    let mut session = Sam3Session::new(server.url.trim_end_matches("/v1")).unwrap();
    session.prepare(&mask(&[255, 255])).unwrap();
    session
}

fn no_views(server: &sam3_support::Server) {
    assert!(
        !server
            .recorded()
            .iter()
            .any(|r| r.path.starts_with("/view"))
    );
}

#[test]
fn a_corrupt_later_mask_fails_instead_of_returning_the_first_instance() {
    let server = server(
        two_masks(),
        vec![
            ("left.png", mask(&[255, 0])),
            ("right.png", b"bad PNG".to_vec()),
        ],
    );
    assert!(prepared(&server).ground("eye").is_err());
    assert!(
        server
            .recorded()
            .iter()
            .any(|r| r.path.contains("filename=right.png"))
    );
}

#[test]
fn a_failed_later_download_does_not_return_a_partial_union() {
    let server = server(two_masks(), vec![("left.png", mask(&[255, 0]))]);
    assert!(prepared(&server).ground("eye").is_err());
}

#[test]
fn a_wrong_sized_later_mask_fails_the_whole_batch() {
    let server = server(
        two_masks(),
        vec![
            ("left.png", mask(&[255, 0])),
            ("right.png", sized_mask(1, 1, &[255])),
        ],
    );
    let error = prepared(&server).ground("eye").unwrap_err();
    assert!(error.message.contains("dimensions"));
}

#[test]
fn unsafe_later_descriptor_is_rejected_before_the_first_download() {
    let mut images = two_masks();
    images[1]["filename"] = json!("../private-mask.png");
    let server = server(images, vec![("left.png", mask(&[255, 0]))]);
    let error = prepared(&server).ground("eye").unwrap_err();
    assert!(error.message.contains("unsafe"));
    assert!(!error.message.contains("private-mask"));
    no_views(&server);
}

#[test]
fn non_output_later_descriptor_is_rejected_before_downloads() {
    let mut images = two_masks();
    images[1]["type"] = json!("input");
    let server = server(images, vec![("left.png", mask(&[255, 0]))]);
    assert!(prepared(&server).ground("eye").is_err());
    no_views(&server);
}

#[test]
fn non_png_later_descriptor_is_rejected_before_downloads() {
    let mut images = two_masks();
    images[1]["filename"] = json!("right.jpg");
    let server = server(images, vec![("left.png", mask(&[255, 0]))]);
    assert!(prepared(&server).ground("eye").is_err());
    no_views(&server);
}

#[test]
fn more_than_one_hundred_masks_is_refused_before_downloads() {
    let descriptor = json!({"filename":"left.png","type":"output","subfolder":""});
    let server = server(
        Value::Array(vec![descriptor; 101]),
        vec![("left.png", mask(&[255, 0]))],
    );
    let error = prepared(&server).ground("eye").unwrap_err();
    assert!(error.message.contains("too many"));
    no_views(&server);
}

#[test]
fn aggregate_decoded_pixel_budget_is_checked_before_downloads() {
    let descriptor = json!({"filename":"left.png","type":"output","subfolder":""});
    let server = server(
        Value::Array(vec![descriptor; 5]),
        vec![("left.png", mask(&[255, 0]))],
    );
    let mut session = Sam3Session::new(server.url.trim_end_matches("/v1")).unwrap();
    session
        .prepare(&sized_mask(4096, 4096, &vec![255; 4096 * 4096]))
        .unwrap();
    let error = session.ground("eye").unwrap_err();
    assert!(error.message.contains("pixel limit"));
    no_views(&server);
}

#[test]
fn aggregate_encoded_byte_budget_is_not_a_per_mask_limit() {
    // A valid tiny PNG followed by padding lets the transport limit be tested without huge images.
    let mut left = mask(&[255, 0]);
    let mut right = mask(&[0, 255]);
    left.resize(16 * 1024 * 1024, 0);
    right.resize(16 * 1024 * 1024 + 1, 0);
    let server = server(two_masks(), vec![("left.png", left), ("right.png", right)]);
    let error = prepared(&server).ground("eye").unwrap_err();
    assert!(error.message.contains("too large") || error.message.contains("byte limit"));
}

#[test]
fn an_empty_first_mask_does_not_hide_a_later_matching_instance() {
    let server = server(
        two_masks(),
        vec![("left.png", mask(&[0, 0])), ("right.png", mask(&[0, 255]))],
    );
    assert_eq!(
        pixels(&prepared(&server).ground("eye").unwrap()),
        vec![0, 255]
    );
}

#[test]
fn all_empty_masks_remain_a_no_match_error() {
    let server = server(
        two_masks(),
        vec![("left.png", mask(&[0, 0])), ("right.png", mask(&[0, 0]))],
    );
    let error = prepared(&server).ground("eye").unwrap_err();
    assert!(error.message.starts_with("Nothing matched"));
}

#[test]
fn empty_output_array_is_not_a_successful_empty_selection() {
    let server = server(json!([]), vec![]);
    assert!(prepared(&server).ground("eye").is_err());
    no_views(&server);
}

#[test]
fn union_preserves_soft_coverage_by_maximum_not_addition() {
    let server = server(
        two_masks(),
        vec![
            ("left.png", mask(&[120, 200])),
            ("right.png", mask(&[150, 80])),
        ],
    );
    assert_eq!(
        pixels(&prepared(&server).ground("eye").unwrap()),
        vec![150, 200]
    );
}
