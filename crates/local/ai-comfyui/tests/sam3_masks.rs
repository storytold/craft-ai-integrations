//! Regression tests for complete, bounded SAM 3 instance-mask retrieval.
mod sam3_support;
use craft_ai_comfyui::Sam3Session;
use sam3_support::{mask, pixels, server, two_masks};

#[test]
fn grounding_unions_every_instance_in_the_mapped_output() {
    let server = server(
        two_masks(),
        vec![
            ("left.png", mask(&[255, 0])),
            ("right.png", mask(&[0, 255])),
        ],
    );
    let mut session = Sam3Session::new(server.url.trim_end_matches("/v1")).unwrap();
    session.prepare(&mask(&[255, 255])).unwrap();
    let selected = session.ground("eye").unwrap();
    assert_eq!(
        pixels(&selected),
        vec![255, 255],
        "both instances must survive retrieval"
    );
    let paths: Vec<_> = server.recorded().into_iter().map(|r| r.path).collect();
    assert!(paths.iter().any(|path| path.contains("filename=right.png")));
}

#[test]
fn grounding_can_preserve_individual_instances_without_merging_them() {
    let server = server(
        two_masks(),
        vec![
            ("left.png", mask(&[255, 0])),
            ("right.png", mask(&[0, 255])),
        ],
    );
    let mut session = Sam3Session::new(server.url.trim_end_matches("/v1")).unwrap();
    session.prepare(&mask(&[255, 255])).unwrap();
    let batch = session.ground_masks("eye").unwrap();
    assert_eq!(batch.dimensions(), (2, 1));
    assert_eq!(batch.masks().len(), 2);
    assert_eq!(pixels(&batch.masks()[0]), vec![255, 0]);
    assert_eq!(pixels(&batch.masks()[1]), vec![0, 255]);
    assert_eq!(pixels(&batch.union_png().unwrap()), vec![255, 255]);
}
