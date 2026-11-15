//! What each fill service's route check says about reference pictures: Gemini and Omni state no
//! count of their own (the service judges it), FLUX.1 Fill takes none. The check reads no key and builds no provider.

use craft_ai_client::{ProviderFactory, RouteQuery, RouteSupport};
use craft_ai_core::model::InputRole;
use craft_ai_core::{CapabilityId, Connection, ModelId};
use serde_json::json;

/// A saved connection for `provider` at `endpoint`.
fn connection(provider: &str, endpoint: &str) -> Connection {
    serde_json::from_value(json!({
        "id": "c", "label": "Fill", "provider": provider, "endpoint": endpoint,
        "locality": "remote", "auth": {"kind": "api_key", "secret": "c-key"},
        "billing": "metered", "models": []
    }))
    .unwrap()
}

/// A fill's inputs with `references` reference pictures after the prompt.
fn fill(references: usize) -> Vec<InputRole> {
    let mut inputs = vec![InputRole::Source, InputRole::Mask, InputRole::Prompt];
    inputs.extend(std::iter::repeat_n(InputRole::Reference, references));
    inputs
}

/// The standard factory's answer for one fill on `model` with `inputs`.
fn ask(connection: &Connection, model: &str, inputs: &[InputRole]) -> RouteSupport {
    ProviderFactory::standard().route_support(&RouteQuery {
        connection,
        model: &ModelId::new(model).unwrap(),
        capability: &CapabilityId::parse("image.inpaint").unwrap(),
        inputs,
        outputs: 1,
        stream: false,
        has_options: false,
    })
}

/// Whether `s` is a refusal.
fn refused(s: &RouteSupport) -> bool {
    matches!(s, RouteSupport::Unsupported { .. })
}

/// Gemini is supported with no references and with many, for a listed and an unlisted model.
#[test]
fn gemini_states_no_reference_count() {
    let c = connection("gemini", "https://generativelanguage.googleapis.com/v1beta");
    for model in ["gemini-3-pro-image", "gemini-3.1-flash-lite-image"] {
        for n in [0, 1, 20] {
            assert_eq!(
                ask(&c, model, &fill(n)),
                RouteSupport::Supported,
                "{model} {n}"
            );
        }
    }
}

/// Omni's Banana models are supported with no references and with many.
#[test]
fn omni_states_no_reference_count() {
    let c = connection("artcraft-omni", "https://api.storyteller.ai");
    for model in ["nano_banana", "nano_banana_2", "nano_banana_pro"] {
        for n in [0, 1, 20] {
            assert_eq!(
                ask(&c, model, &fill(n)),
                RouteSupport::Supported,
                "{model} {n}"
            );
        }
    }
}

/// FLUX.1 Fill takes no reference at all; the plain fill stays supported.
#[test]
fn flux_fill_refuses_any_reference() {
    let c = connection("bfl", "https://api.bfl.ai/v1");
    assert_eq!(
        ask(&c, "flux-pro-1.0-fill", &fill(0)),
        RouteSupport::Supported
    );
    assert!(refused(&ask(&c, "flux-pro-1.0-fill", &fill(1))));
}
