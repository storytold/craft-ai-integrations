//! Gemini's route check: Generative fill only, with the model rule `submit` uses, at the official
//! address or a local test server. Nothing here builds a provider or reads a key.

use craft_ai_client::{ProviderFactory, RouteQuery, RouteSupport};
use craft_ai_core::model::InputRole;
use craft_ai_core::{CapabilityId, Connection, ModelId};
use serde_json::json;

/// A saved Gemini connection at `endpoint`.
fn gemini_at(endpoint: &str) -> Connection {
    serde_json::from_value(json!({
        "id": "g",
        "label": "Gemini",
        "provider": "gemini",
        "endpoint": endpoint,
        "locality": "remote",
        "auth": {"kind": "api_key", "secret": "g-key"},
        "billing": "metered",
        "models": []
    }))
    .unwrap()
}

/// What the standard factory says about one fill request.
fn ask(
    connection: &Connection,
    model: &str,
    capability: &str,
    inputs: &[InputRole],
    outputs: u32,
) -> RouteSupport {
    ProviderFactory::standard().route_support(&RouteQuery {
        connection,
        model: &ModelId::new(model).unwrap(),
        capability: &CapabilityId::parse(capability).unwrap(),
        inputs,
        outputs,
        stream: false,
        has_options: true,
    })
}

const OFFICIAL: &str = "https://generativelanguage.googleapis.com/v1beta";
const FILL: &[InputRole] = &[InputRole::Source, InputRole::Mask, InputRole::Prompt];

/// A listed model, a newer well-formed one, and a fill without a prompt are all supported, options
/// included, because `submit` takes all of them.
#[test]
fn gemini_fill_is_supported_for_any_well_formed_image_model() {
    let c = gemini_at(OFFICIAL);
    for model in ["gemini-3-pro-image", "gemini-9-ultra-image-preview"] {
        assert_eq!(
            ask(&c, model, "image.inpaint", FILL, 1),
            RouteSupport::Supported,
            "{model}"
        );
    }
    let no_prompt = [InputRole::Source, InputRole::Mask];
    assert_eq!(
        ask(&c, "gemini-3-pro-image", "image.inpaint", &no_prompt, 1),
        RouteSupport::Supported
    );
}

/// Each thing `submit` would refuse is refused here first.
#[test]
fn gemini_refuses_what_submit_would_refuse() {
    let c = gemini_at(OFFICIAL);
    let model = "gemini-3-pro-image";
    let refused = |s: RouteSupport| matches!(s, RouteSupport::Unsupported { .. });
    assert!(
        refused(ask(&c, "gpt-image-1", "image.inpaint", FILL, 1)),
        "not a Gemini image model"
    );
    assert!(
        refused(ask(&c, model, "image.generate", FILL, 1)),
        "not a fill"
    );
    assert!(
        refused(ask(&c, model, "image.inpaint", FILL, 2)),
        "two outputs"
    );
    assert!(
        refused(ask(&c, model, "image.inpaint", &[InputRole::Source], 1)),
        "no mask"
    );
    let two_masks = [InputRole::Source, InputRole::Mask, InputRole::Mask];
    assert!(
        refused(ask(&c, model, "image.inpaint", &two_masks, 1)),
        "two masks"
    );
    let with_reference = [InputRole::Source, InputRole::Mask, InputRole::Reference];
    assert!(
        refused(ask(&c, model, "image.inpaint", &with_reference, 1)),
        "reference"
    );
}

/// A saved address the constructor would refuse is reported, and a local test server is allowed.
#[test]
fn gemini_address_rule_matches_its_constructor() {
    let model = "gemini-3-pro-image";
    let other = gemini_at("https://example.com/v1beta");
    assert!(matches!(
        ask(&other, model, "image.inpaint", FILL, 1),
        RouteSupport::Unsupported { .. }
    ));
    let local = gemini_at("http://127.0.0.1:9/v1beta");
    assert_eq!(
        ask(&local, model, "image.inpaint", FILL, 1),
        RouteSupport::Supported
    );
}
