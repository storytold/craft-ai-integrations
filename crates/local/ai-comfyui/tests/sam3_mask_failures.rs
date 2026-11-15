//! Safety regressions for complete SAM 3 batches. A bad later result must fail the operation.
mod sam3_support;
use craft_ai_comfyui::Sam3Session;
use sam3_support::{mask, server, two_masks};
use serde_json::json;

#[test]
fn wrong_typed_later_subfolder_is_rejected_before_any_mask_download() {
    let mut images = two_masks();
    images[1]["subfolder"] = json!(true);
    let server = server(
        images,
        vec![
            ("left.png", mask(&[255, 0])),
            ("right.png", mask(&[0, 255])),
        ],
    );
    let mut session = Sam3Session::new(server.url.trim_end_matches("/v1")).unwrap();
    session.prepare(&mask(&[255, 255])).unwrap();
    assert!(session.ground("eye").is_err());
    assert!(
        !server
            .recorded()
            .iter()
            .any(|r| r.path.starts_with("/view"))
    );
}
