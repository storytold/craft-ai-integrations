//! The SAM 2.1 graph and the catalogue must agree on the detector, or setup would install one
//! folder while the session refuses to run without another.

/// The folder the graph asks Florence-2 for is the folder the catalogue installs.
#[test]
fn the_graph_and_the_catalogue_name_the_same_detector_folder() {
    assert_eq!(
        craft_ai_comfyui::FLORENCE2_DETECTOR,
        craft_ai_features::FLORENCE2_LARGE_DIR
    );
}
