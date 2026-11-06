//! Reference pictures through the shared client: plan, consent and start for Omni (which takes
//! them) and FLUX.1 Fill (which does not), with the selected model kept and nothing sent early.
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;
use craft_ai_artcraft::{ArtCraftConfig, ArtCraftProvider};
use craft_ai_bfl::BflProvider;
use craft_ai_client::{AiClient, ProviderFactory};
use craft_ai_core::{
    AppId, Binding, Connection, ConsentLedger, ErrorKind, InputRole, ModelId, ModelInfo, Payload,
    Provider, Request, Role, SecretRef, SecretString, StoreFile,
};
use craft_ai_native::{
    secrets::{MemoryStore, SecretStore},
    shared::SharedStore,
};
use std::sync::Arc;
use support::{Reply, Server};

/// An 8 x 8 RGB PNG of one colour.
fn png_rgb(rgb: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, 8, 8);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&rgb.repeat(64)).unwrap();
    writer.finish().unwrap();
    out
}

/// The built-in Generative fill role.
fn fill_role() -> Role {
    craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == "generative-fill")
        .unwrap()
        .role()
        .unwrap()
}

/// A fill with a prompt and one reference picture.
fn fill_with_reference(role: &Role) -> Request {
    Request::new(role.capability.clone())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", png_rgb([10, 20, 30])).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", png_rgb([255, 255, 255])).unwrap(),
        )
        .with_input(InputRole::Prompt, Payload::text("a blue vase"))
        .with_input(
            InputRole::Reference,
            Payload::inline("image/png", png_rgb([200, 0, 0])).unwrap(),
        )
}

/// A client for PhotoCraft whose fill role is bound to `model` on a saved connection for
/// `provider` with `models`, and whose `provider` adapter is built against `server`.
fn client(
    provider: &str,
    endpoint: &str,
    models: Vec<ModelInfo>,
    model: &str,
    server: &Server,
    dir: &tempfile::TempDir,
) -> AiClient {
    let role = fill_role();
    let connection: Connection = serde_json::from_value(serde_json::json!({
        "id": "fill-refs", "label": "Fill", "provider": provider, "endpoint": endpoint,
        "locality": "remote", "auth": {"kind": "api_key", "secret": "fill-refs-key"},
        "billing": "metered", "models": models
    }))
    .unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    shared
        .replace(&StoreFile {
            connections: vec![connection.clone()],
            defaults: vec![Binding {
                role: role.id.clone(),
                connection: connection.id,
                model: ModelId::new(model).unwrap(),
            }],
            ..StoreFile::default()
        })
        .unwrap();
    let secrets = Arc::new(MemoryStore::new());
    secrets
        .set(
            &SecretRef::new("fill-refs-key").unwrap(),
            &SecretString::new("fixture-only-key"),
        )
        .unwrap();
    let mut factory = ProviderFactory::standard();
    let base = server.url.trim_end_matches("/v1").to_owned();
    factory.register("artcraft-omni", move |_, key| {
        Ok(Arc::new(ArtCraftProvider::new(ArtCraftConfig::new(
            &base,
            key.unwrap(),
        ))?) as Arc<dyn Provider>)
    });
    AiClient::new(AppId::new("photocraft").unwrap(), shared, secrets).with_factory(factory)
}

/// Omni: the disclosure lists source, mask, prompt and reference apart and in request order,
/// nothing is uploaded before consent, and after it the reference is the third upload.
#[test]
fn omni_fill_with_a_reference_discloses_it_and_waits_for_consent() {
    let server = Server::start(|r| match r.path.as_str() {
        "/v1/omni_api/upload/image" => {
            Reply::Json(200, r#"{"success":true,"media_file_token":"m_x"}"#.into())
        }
        "/v1/omni_api/generate/image" => Reply::Json(
            200,
            r#"{"success":true,"inference_job_token":"jinf_refs"}"#.into(),
        ),
        _ => Reply::Json(
            200,
            r#"{"success":true,"state":{"job_token":"jinf_refs","status":{"status":"pending"}}}"#
                .into(),
        ),
    });
    let dir = tempfile::tempdir().unwrap();
    let models = ArtCraftProvider::documented_models();
    let c = client(
        "artcraft-omni",
        "https://api.storyteller.ai",
        models,
        "nano_banana_2",
        &server,
        &dir,
    );
    let role = fill_role();
    let plan = c.plan(&role, fill_with_reference(&role)).unwrap();
    assert_eq!(plan.model().as_str(), "nano_banana_2");
    let roles: Vec<_> = plan
        .disclosure()
        .items
        .iter()
        .map(|i| i.role.clone())
        .collect();
    assert_eq!(
        roles,
        [
            InputRole::Source,
            InputRole::Mask,
            InputRole::Prompt,
            InputRole::Reference
        ]
    );
    let mut consent = ConsentLedger::default();
    assert_eq!(
        c.start(&plan, &consent).unwrap_err().kind,
        ErrorKind::ConsentRequired
    );
    assert_eq!(server.count(), 0, "no uploads before consent");
    consent.grant(plan.disclosure());
    c.start(&plan, &consent).unwrap();
    let sent = server.recorded();
    let uploads: Vec<_> = sent
        .iter()
        .filter(|r| r.path.ends_with("upload/image"))
        .collect();
    assert_eq!(uploads.len(), 3);
    let reference = png_rgb([200, 0, 0]);
    assert!(
        uploads[2]
            .body
            .windows(reference.len())
            .any(|w| w == reference.as_slice())
    );
}

/// FLUX.1 Fill: a reference is refused at plan time, before anyone is asked to agree, and nothing
/// is sent.
#[test]
fn flux_fill_with_a_reference_is_refused_before_consent() {
    let server = Server::start(|_| Reply::Json(500, "unexpected".into()));
    let dir = tempfile::tempdir().unwrap();
    let c = client(
        "bfl",
        "https://api.bfl.ai/v1",
        BflProvider::catalog(),
        "flux-pro-1.0-fill",
        &server,
        &dir,
    );
    let role = fill_role();
    assert!(c.plan(&role, fill_with_reference(&role)).is_err());
    assert_eq!(server.count(), 0);
}
