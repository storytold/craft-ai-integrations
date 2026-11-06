//! Pins the built-in catalogue so an empty or broken list cannot ship, and checks the
//! normalisation into MCP-listable descriptors.

use craft_ai_mcp::catalogue::{app_names, tools};

#[test]
fn pins_the_builtin_catalogue() {
    let tools = tools();
    assert_eq!(tools.len(), 8);
    assert_eq!(
        tools.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
        vec![
            "select-subject".to_string(),
            "remove-background".to_string(),
            "remove-object".to_string(),
            "generative-fill".to_string(),
            "generative-expand".to_string(),
            "extend-sequence".to_string(),
            "generate-image".to_string(),
            "generate-video".to_string(),
        ],
    );
    assert_eq!(
        tools
            .iter()
            .map(|t| t.capability.clone())
            .collect::<Vec<_>>(),
        vec![
            "image.segment".to_string(),
            "image.matte".to_string(),
            "image.inpaint".to_string(),
            "image.inpaint".to_string(),
            "image.outpaint".to_string(),
            "video.generate".to_string(),
            "image.generate".to_string(),
            "video.generate".to_string(),
        ],
    );
}

#[test]
fn every_tool_has_metadata_and_expected_apps() {
    for t in tools() {
        assert!(!t.id.is_empty());
        assert!(!t.title.is_empty());
        assert!(!t.capability.is_empty());
        if t.id == "select-subject" {
            assert!(t.apps.iter().any(|a| a.app == "PhotoCraft"));
            assert!(t.apps.iter().any(|a| a.app == "LightCraft"));
        }
    }
}

#[test]
fn app_names_are_the_catalogue_apps() {
    let names = app_names();
    for n in [
        "PhotoCraft",
        "LightCraft",
        "EffectCraft",
        "FilmCraft",
        "DesignCraft",
        "DeckCraft",
    ] {
        assert!(names.contains(&n.to_string()), "missing catalogue app {n}");
    }
}
