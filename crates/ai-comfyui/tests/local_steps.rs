//! Naming the steps of an imported workflow that are not known to keep data on this computer.
//! Pure checks on graphs; no server is involved.
use craft_ai_comfyui::local_steps::{KNOWN_LOCAL_STEPS, unknown_steps};
use serde_json::json;

#[test]
fn comfyuis_own_local_steps_are_not_reported() {
    let graph = json!({
        "1": {"class_type": "CheckpointLoaderSimple", "inputs": {}},
        "2": {"class_type": "CLIPTextEncode", "inputs": {}},
        "3": {"class_type": "KSampler", "inputs": {}},
        "4": {"class_type": "VAEDecode", "inputs": {}},
        "5": {"class_type": "SaveImage", "inputs": {}}
    });
    assert!(unknown_steps(&graph).is_empty());
}

#[test]
fn every_other_step_is_named_once_in_order() {
    let graph = json!({
        "1": {"class_type": "LoadImage", "inputs": {}},
        "2": {"class_type": "WebhookSend", "inputs": {}},
        "3": {"class_type": "HttpPost", "inputs": {}},
        "4": {"class_type": "HttpPost", "inputs": {}},
        "5": {"class_type": "loadimage", "inputs": {}}
    });
    assert_eq!(
        unknown_steps(&graph),
        ["HttpPost", "WebhookSend", "loadimage"]
    );
}

#[test]
fn a_step_without_a_readable_name_is_still_reported() {
    let graph = json!({
        "1": {"inputs": {}},
        "2": {"class_type": 7, "inputs": {}},
        "3": {"class_type": "  ", "inputs": {}}
    });
    assert_eq!(unknown_steps(&graph), ["", "  "]);
}

#[test]
fn reported_names_are_exact_however_long_or_odd() {
    // Consent compares these names, so nothing is cut or cleaned: two steps that share their
    // first 80 characters stay two steps.
    let shared = "A".repeat(80);
    let graph = json!({
        "1": {"class_type": format!("{shared}One"), "inputs": {}},
        "2": {"class_type": format!("{shared}Two"), "inputs": {}},
        "3": {"class_type": "Send\nTo\u{7}Host", "inputs": {}}
    });
    assert_eq!(
        unknown_steps(&graph),
        [
            format!("{shared}One"),
            format!("{shared}Two"),
            "Send\nTo\u{7}Host".to_owned()
        ]
    );
}

#[test]
fn the_known_list_names_no_step_that_sends_to_a_hosted_service() {
    // ComfyUI's own hosted-service steps send their inputs to another company's servers; none
    // may be on the list. Their names carry the service's name.
    for step in KNOWN_LOCAL_STEPS {
        let lower = step.to_ascii_lowercase();
        for service in [
            "openai",
            "gemini",
            "kling",
            "luma",
            "ideogram",
            "recraft",
            "stability",
            "api",
            "http",
            "url",
            "upload",
        ] {
            assert!(!lower.contains(service), "{step}");
        }
    }
}
