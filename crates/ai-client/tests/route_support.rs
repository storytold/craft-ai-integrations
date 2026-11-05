//! A known model the adapter would refuse is reported before consent, and nothing is sent or read.
//! Saved facts, what this build can run, and what has really worked stay separate.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use craft_ai_client::{AiClient, ProviderFactory, RoleStatus, RouteSupport};
use craft_ai_core::model::InputRole;
use craft_ai_core::picks::CatalogEntry;
use craft_ai_core::{AppId, Catalog, ModelId};
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;

#[path = "support/route_fixture.rs"]
mod route_fixture;
use route_fixture::*;

/// A model the Omni list does not serve is not ready, though the saved facts still resolve.
#[test]
fn a_saved_omni_model_the_adapter_does_not_serve_is_not_ready() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "artcraft-omni", "not_a_real_model");
    let c = client(shared, ProviderFactory::standard());
    assert!(matches!(c.status(&role()), RoleStatus::Unavailable { .. }));
    let facts = c.route_facts(&role());
    assert_eq!(facts.configured, Ok(()), "the saved facts still resolve");
    assert!(matches!(facts.executable, RouteSupport::Unsupported { .. }));
    assert_eq!(facts.last_success_ms, None);
}

/// Plan refuses it before consent, and no other model is chosen in its place.
#[test]
fn it_is_refused_at_plan_before_consent_and_stays_chosen() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "artcraft-omni", "not_a_real_model");
    let before = std::fs::read(shared.path()).unwrap();
    let c = client(shared.clone(), ProviderFactory::standard());
    let refused = c.plan(&role(), request()).unwrap_err();
    assert_eq!(refused.kind, craft_ai_core::ErrorKind::Unsupported);
    assert_eq!(
        std::fs::read(shared.path()).unwrap(),
        before,
        "no other model was chosen"
    );
}

/// Status reports a declared refusal without reading a key or building the provider.
#[test]
fn a_status_check_reads_no_key_and_contacts_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "custom-refuses", "any");
    let mut factory = ProviderFactory::standard();
    factory.register("custom-refuses", |_, _| {
        panic!("asking about support must not build a provider")
    });
    factory.declare_support("custom-refuses", |_| RouteSupport::Unsupported {
        reason: "Not this one.".to_owned(),
    });
    let reads = Arc::new(AtomicUsize::new(0));
    let secrets = Counting(MemoryStore::new(), reads.clone());
    let c = AiClient::new(AppId::new("photocraft").unwrap(), shared, Arc::new(secrets))
        .with_factory(factory);
    assert_eq!(
        c.status(&role()),
        RoleStatus::Unavailable {
            reason: "Not this one.".to_owned()
        }
    );
    assert_eq!(reads.load(Ordering::SeqCst), 0, "no key was read");
}

/// An adapter that declares nothing is untested, and is ready and plans as before.
#[test]
fn an_adapter_that_does_not_say_is_untested_and_still_runs() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "custom-quiet", "any");
    let mut factory = ProviderFactory::standard();
    factory.register("custom-quiet", |_, _| Ok(Arc::new(Untouched)));
    let c = client(shared, factory);
    assert!(matches!(c.status(&role()), RoleStatus::Ready(_)));
    assert_eq!(c.route_facts(&role()).executable, RouteSupport::Untested);
    assert!(c.plan(&role(), request()).is_ok());
}

/// An adapter registered under a built-in name drops the built-in list check.
#[test]
fn a_custom_adapter_replacing_a_built_in_name_is_not_judged_by_its_list() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "artcraft-omni", "my_own_model");
    let mut factory = ProviderFactory::standard();
    factory.register("artcraft-omni", |_, _| Ok(Arc::new(Untouched)));
    let c = client(shared, factory);
    assert_eq!(c.route_facts(&role()).executable, RouteSupport::Untested);
    assert!(matches!(c.status(&role()), RoleStatus::Ready(_)));
}

/// A custom adapter can declare a model supported.
#[test]
fn a_custom_adapter_can_say_it_supports_a_model() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "custom-sure", "any");
    let mut factory = ProviderFactory::standard();
    factory.register("custom-sure", |_, _| Ok(Arc::new(Untouched)));
    factory.declare_support("custom-sure", |_| RouteSupport::Supported);
    let c = client(shared, factory);
    assert_eq!(c.route_facts(&role()).executable, RouteSupport::Supported);
}

/// A refused model is left out of choices and picks, even when the catalogue recommends it,
/// and binding it is refused with nothing written.
#[test]
fn a_model_known_to_be_refused_is_not_offered_or_bound() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "artcraft-omni", "not_a_real_model");
    // The catalogue recommends the refused model, so `picks` would offer it without its filter.
    let catalog = Catalog {
        version: 1,
        entries: vec![CatalogEntry {
            capability: "image.generate".into(),
            pick: "recommended".into(),
            provider: "artcraft-omni".into(),
            model: "not_a_real_model".into(),
        }],
    };
    let c = client(shared.clone(), ProviderFactory::standard()).with_catalog(catalog);
    assert!(c.choices(&role()).unwrap().is_empty());
    assert!(c.picks(&role()).unwrap().is_empty());
    let before = std::fs::read(shared.path()).unwrap();
    let refused = c.bind(
        &role(),
        &"one".parse().unwrap(),
        &ModelId::new("not_a_real_model").unwrap(),
    );
    assert!(refused.is_err());
    assert_eq!(std::fs::read(shared.path()).unwrap(), before);
}

/// A tool with no choice reports that, and says nothing about whether anything would run.
#[test]
fn an_unconfigured_role_says_so_and_claims_nothing_about_running() {
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let c = client(shared, ProviderFactory::standard());
    let facts = c.route_facts(&role());
    assert!(facts.configured.is_err());
    assert_eq!(facts.executable, RouteSupport::Untested);
}

/// A tool input the listed model does not take makes the route unsupported.
#[test]
fn inputs_a_listed_model_does_not_take_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "artcraft-omni", "nano_banana");
    let c = client(shared, ProviderFactory::standard());
    assert_eq!(c.route_facts(&role()).executable, RouteSupport::Supported);
    let mut with_mask = role();
    with_mask.inputs.push(InputRole::Mask);
    assert!(matches!(
        c.route_facts(&with_mask).executable,
        RouteSupport::Unsupported { .. }
    ));
}

/// Vertex express is supported for a prompt alone, not with a source image.
#[test]
fn vertex_takes_only_a_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "vertex-express", "gemini-2.5-flash-image");
    let c = client(shared, ProviderFactory::standard());
    assert_eq!(c.route_facts(&role()).executable, RouteSupport::Supported);
    let mut with_image = role();
    with_image.inputs.push(InputRole::Source);
    assert!(matches!(
        c.route_facts(&with_image).executable,
        RouteSupport::Unsupported { .. }
    ));
}
