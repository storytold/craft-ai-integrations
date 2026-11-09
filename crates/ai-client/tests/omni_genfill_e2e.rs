//! Catalogue, saved binding, consent and real Omni adapter through one complete fill.
#[path = "../../ai-openai/tests/support/mod.rs"]
mod support;
use craft_ai_artcraft::{ArtCraftConfig, ArtCraftProvider};
use craft_ai_client::{AiClient, ProviderFactory, RoleStatus};
use craft_ai_core::{
    AppId, Binding, CapabilitySpec, Connection, ConsentLedger, ErrorKind, InputRole, JobState,
    ModelId, ModelInfo, Payload, Provider, Request, Role, SecretRef, SecretString, StoreFile,
};
use craft_ai_native::{
    secrets::{MemoryStore, SecretStore},
    shared::SharedStore,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use support::{Reply, Server};

/// Eight synthetic pixels in either dimension, never a personal image.
fn png() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, 8, 8);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[255; 8 * 8 * 4])
            .unwrap();
    }
    out
}

/// The selected provider/model stay unchanged; only its transport address is isolated.
#[test]
fn selected_omni_fill_runs_from_plan_and_consent_to_inline_result() {
    let uploads = Arc::new(AtomicUsize::new(0));
    let count = uploads.clone();
    let server = Server::start(move |r| match (r.method.as_str(), r.path.as_str()) {
        ("POST", "/v1/omni_api/upload/image") => {
            let n = count.fetch_add(1, Ordering::SeqCst);
            Reply::Json(
                200,
                format!(r#"{{"success":true,"media_file_token":"m_input_{n}"}}"#),
            )
        }
        ("POST", "/v1/omni_api/generate/image") => {
            assert_eq!(r.body_json()["model"], "nano_banana_2");
            assert_eq!(r.body_json()["image_batch_count"], 1);
            Reply::Json(
                200,
                r#"{"success":true,"inference_job_token":"jinf_e2e"}"#.into(),
            )
        }
        ("GET", "/v1/omni_api/job_status/job/jinf_e2e") => Reply::Json(
            200,
            serde_json::json!({"success":true,"state":{"job_token":"jinf_e2e",
                "status":{"status":"complete_success"}, "maybe_result":{"media_links":{
                    "cdn_url":format!("http://{}/result.png", r.header("host").unwrap())}}}})
            .to_string(),
        ),
        ("GET", "/result.png") => {
            assert!(r.header("authorization").is_none());
            let data = png();
            let mut raw = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                data.len()
            )
            .into_bytes();
            raw.extend_from_slice(&data);
            Reply::Raw(raw)
        }
        _ => Reply::Json(500, "unexpected HTTP request".into()),
    });
    let role: Role = craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == "generative-fill")
        .unwrap()
        .role()
        .unwrap();
    let model = ModelInfo::new(ModelId::new("nano_banana_2").unwrap(), "Nano Banana 2").with(
        CapabilitySpec::new(role.capability.clone())
            .accepting(InputRole::Source, 1)
            .requiring(InputRole::Source)
            .accepting(InputRole::Mask, 1)
            .requiring(InputRole::Mask)
            .accepting(InputRole::Prompt, 1)
            .requiring(InputRole::Prompt),
    );
    let connection: Connection = serde_json::from_value(serde_json::json!({
        "id":"omni-e2e", "label":"ArtCraft Omni test", "provider":"artcraft-omni",
        "endpoint":"https://api.storyteller.ai", "locality":"remote",
        "auth":{"kind":"api_key","secret":"omni-e2e-fixture"}, "billing":"metered", "models":[model]
    }))
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    shared
        .replace(&StoreFile {
            connections: vec![connection.clone()],
            defaults: vec![Binding {
                role: role.id.clone(),
                connection: connection.id,
                model: ModelId::new("nano_banana_2").unwrap(),
            }],
            ..StoreFile::default()
        })
        .unwrap();
    let secrets = Arc::new(MemoryStore::new());
    secrets
        .set(
            &SecretRef::new("omni-e2e-fixture").unwrap(),
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
    let client =
        AiClient::new(AppId::new("photocraft").unwrap(), shared, secrets).with_factory(factory);
    assert!(matches!(client.status(&role), RoleStatus::Ready(_)));
    let request = Request::new(role.capability.clone())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", png()).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", png()).unwrap(),
        )
        .with_input(InputRole::Prompt, Payload::text("a small red balloon"));
    let plan = client.plan(&role, request).unwrap();
    assert_eq!(plan.provider(), "artcraft-omni");
    assert_eq!(plan.model().as_str(), "nano_banana_2");
    assert_eq!(plan.disclosure().items.len(), 3);
    let mut consent = ConsentLedger::default();
    assert_eq!(
        client.start(&plan, &consent).unwrap_err().kind,
        ErrorKind::ConsentRequired
    );
    assert_eq!(server.count(), 0, "no uploads before consent");
    consent.grant(plan.disclosure());
    let done = client
        .start(&plan, &consent)
        .unwrap()
        .wait(Duration::from_millis(1), Duration::from_secs(3))
        .unwrap();
    assert_eq!(done.state, JobState::Succeeded);
    assert!(
        done.events
            .iter()
            .any(|e| matches!(e, craft_ai_core::Event::Output(o)
        if matches!(&o.payload, Payload::Inline {data, ..} if *data == png())))
    );
    assert_eq!(uploads.load(Ordering::SeqCst), 2);
    assert_eq!(server.count(), 5);
    println!(
        "PASS: selected Nano Banana 2 -> plan -> consent -> two private uploads -> one generation -> inline PNG"
    );
}
