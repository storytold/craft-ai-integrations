//! A key save refused because the secret file needs a passphrase reaches the person with that
//! reason, not the generic "keychain failed" advice. Nothing here touches the OS keychain.

use std::sync::Arc;

use craft_ai_client::{ConnectionManager, SaveError};
use craft_ai_core::{AiError, ConnectionDraft, ErrorKind, PRESETS, SecretRef, SecretString};
use craft_ai_native::secrets::SecretStore;
use craft_ai_native::shared::SharedStore;

const KEY: &str = "sk-live-key-12345";
const ASK: &str = "choose a passphrase to keep keys in an encrypted file";

/// A store that refuses every save with a chosen kind, as the locked secret file does.
struct Refuses(ErrorKind);

impl SecretStore for Refuses {
    /// Names itself like the secret file.
    fn name(&self) -> &'static str {
        "encrypted file"
    }
    /// Holds nothing.
    fn get(&self, _: &SecretRef) -> Result<Option<SecretString>, AiError> {
        Ok(None)
    }
    /// Refuses with the reason the locked file gives.
    fn set(&self, _: &SecretRef, _: &SecretString) -> Result<(), AiError> {
        Err(AiError::new(self.0, ASK))
    }
    /// Nothing to remove.
    fn delete(&self, _: &SecretRef) -> Result<(), AiError> {
        Ok(())
    }
}

/// An OpenAI connection draft carrying `KEY`.
fn openai() -> ConnectionDraft {
    let mut d = ConnectionDraft::from_preset(PRESETS.iter().find(|p| p.key == "openai").unwrap());
    d.key = KEY.to_owned();
    d
}

/// Saves through a manager over a store that refuses with `kind`, returning the error shown.
fn refused_save(kind: ErrorKind) -> AiError {
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let manager = ConnectionManager::new(shared, Arc::new(Refuses(kind)));
    let SaveError::Failed(e) = manager.save(&openai()).unwrap_err() else {
        panic!("expected a storage failure");
    };
    assert!(manager.views().unwrap().is_empty());
    assert!(!e.message.contains(KEY));
    e
}

/// A save the store refuses as a credentials problem shows the store's own reason.
#[test]
fn a_save_that_needs_a_passphrase_says_so() {
    let e = refused_save(ErrorKind::Auth);
    assert_eq!(e.kind, ErrorKind::Auth);
    assert!(e.message.contains(ASK), "{}", e.message);
}

/// Any other refusal keeps the general advice, which names the environment variable.
#[test]
fn other_refusals_keep_the_general_advice() {
    let e = refused_save(ErrorKind::Internal);
    assert!(e.message.contains("CRAFT_AI_SECRET_"), "{}", e.message);
}
