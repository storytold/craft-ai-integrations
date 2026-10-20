//! The app-facing picker, status, binding and planning must agree on required inputs.
//! Scripted store only: no provider, keychain or network is contacted.
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use craft_ai_client::{AiClient, RoleStatus};
use craft_ai_core::{
    AiError, AppId, Auth, Binding, CapabilityId, CapabilitySpec, ConnectionDraft, ErrorKind,
    InputRole, ModelId, ModelInfo, Payload, Request, Role, RoleId, SecretRef, SecretString,
    StoreFile,
};
use craft_ai_native::secrets::SecretStore;
use craft_ai_native::shared::SharedStore;

#[derive(Default)]
struct UntouchedSecrets(AtomicUsize);

impl SecretStore for UntouchedSecrets {
    fn name(&self) -> &'static str {
        "untouched"
    }
    fn get(&self, _: &SecretRef) -> Result<Option<SecretString>, AiError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
    fn set(&self, _: &SecretRef, _: &SecretString) -> Result<(), AiError> {
        panic!("an incompatible model must never write a key")
    }
    fn delete(&self, _: &SecretRef) -> Result<(), AiError> {
        panic!("an incompatible model must never delete a key")
    }
}

struct World {
    _dir: tempfile::TempDir,
    shared: SharedStore,
    role: Role,
    binding: Binding,
    secrets: Arc<UntouchedSecrets>,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let capability = CapabilityId::parse("image.inpaint").unwrap();
        let role = Role {
            id: RoleId::new("source-only").unwrap(),
            label: "Source only".into(),
            capability: capability.clone(),
            inputs: vec![InputRole::Source],
        };
        let mut draft = ConnectionDraft::custom();
        draft.label = "Test".into();
        draft.endpoint = "https://service.example/v1".into();
        draft.uses_key = false;
        let mut connection = draft.prepare(&[]).unwrap().connection;
        connection.auth = Auth::ApiKey {
            secret: SecretRef::new("unused-key").unwrap(),
        };
        connection.models = vec![
            ModelInfo::new(ModelId::new("needs-mask").unwrap(), "Needs mask").with(
                CapabilitySpec::new(capability)
                    .accepting(InputRole::Source, 1)
                    .accepting(InputRole::Mask, 1)
                    .requiring(InputRole::Source)
                    .requiring(InputRole::Mask),
            ),
        ];
        let binding = Binding {
            role: role.id.clone(),
            connection: connection.id.clone(),
            model: connection.models[0].id.clone(),
        };
        let shared = SharedStore::new(dir.path().join("connections.json"));
        shared
            .replace(&StoreFile {
                connections: vec![connection],
                defaults: vec![binding.clone()],
                ..StoreFile::default()
            })
            .unwrap();
        Self {
            _dir: dir,
            shared,
            role,
            binding,
            secrets: Arc::new(UntouchedSecrets::default()),
        }
    }

    fn client(&self) -> AiClient {
        AiClient::new(
            AppId::new("test-app").unwrap(),
            self.shared.clone(),
            self.secrets.clone(),
        )
    }
}

#[test]
fn picker_omits_the_model_and_binding_refuses_it_without_writing() {
    let world = World::new();
    let client = world.client();
    let before = std::fs::read(world.shared.path()).unwrap();
    assert!(client.choices(&world.role).unwrap().is_empty());
    assert!(
        client
            .bind(&world.role, &world.binding.connection, &world.binding.model)
            .is_err()
    );
    assert_eq!(std::fs::read(world.shared.path()).unwrap(), before);
    assert_eq!(world.secrets.0.load(Ordering::SeqCst), 0);
}

#[test]
fn status_and_plan_refuse_before_any_key_is_read_or_consent_is_requested() {
    let world = World::new();
    let client = world.client();
    let RoleStatus::Unavailable { reason } = client.status(&world.role) else {
        panic!("the missing tool input cannot be ready")
    };
    assert!(reason.contains("requires a 'mask' input"), "{reason}");
    // Even a request that supplies the mask must not hide the mismatch in the tool's contract.
    let request = Request::new(world.role.capability.clone())
        .with_input(
            InputRole::Source,
            Payload::inline("image/png", vec![1]).unwrap(),
        )
        .with_input(
            InputRole::Mask,
            Payload::inline("image/png", vec![2]).unwrap(),
        );
    let error = client.plan(&world.role, request).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Unsupported);
    assert_eq!(error.message, reason);
    assert_eq!(world.secrets.0.load(Ordering::SeqCst), 0);
}
