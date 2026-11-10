//! The standard secret chain the launcher and every app build, and the fallback for a machine
//! with no OS keychain.
//!
//! Order: environment (read only), OS keychain, then the passphrase-encrypted file at
//! [`paths::secrets_path`](crate::paths::secrets_path). The file layer is always in the chain when
//! the OS reports a config folder, so a key saved there is never silently skipped:
//!
//! - given a passphrase, it is an [`EncryptedFileStore`];
//! - given none, it is a locked stand-in. A missing file reads as "no key here". An existing file
//!   reports that it is locked rather than pretending the key is absent, and a save says a
//!   passphrase is needed. Where a keychain exists, saves land there first and never reach it.
//!
//! [`passphrase_need`] tells a host whether to ask for a passphrase before building the chain.
//! This module never stores a passphrase.

use std::path::PathBuf;

use craft_ai_core::{AiError, ErrorKind, SecretRef, SecretString};

use super::{EncryptedFileStore, EnvStore, KdfParams, KeychainStore, LayeredStore, SecretStore};

/// Whether a host must ask the person for the secret file's passphrase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassphraseNeed {
    /// The OS keychain holds keys and there is no secret file to open.
    NotNeeded,
    /// No keychain and no secret file yet: ask for a new passphrase before the first key is saved.
    Create,
    /// A secret file exists: ask for its passphrase, or keys saved in it cannot be read.
    Unlock,
}

/// What this machine needs, from the real keychain and the default secret file location.
pub fn passphrase_need() -> PassphraseNeed {
    need_for(
        KeychainStore::is_available(),
        crate::paths::secrets_path().map(|path| path.exists()),
    )
}

/// The rule behind [`passphrase_need`]. `file` is `None` when the OS reports no config folder, so
/// there is nowhere to keep a secret file and asking for a passphrase would achieve nothing.
fn need_for(keychain: bool, file: Option<bool>) -> PassphraseNeed {
    match file {
        Some(true) => PassphraseNeed::Unlock,
        Some(false) if !keychain => PassphraseNeed::Create,
        _ => PassphraseNeed::NotNeeded,
    }
}

/// Environment, then OS keychain, then the encrypted file at the default location, unlocked with
/// `passphrase` when one is given. An empty passphrase counts as none.
pub fn standard_store(passphrase: Option<SecretString>) -> LayeredStore {
    chain(
        Box::new(EnvStore::new()),
        Box::new(KeychainStore::new()),
        crate::paths::secrets_path(),
        passphrase,
        KdfParams::default(),
    )
}

/// [`standard_store`] with every part supplied, so tests can stand in for the keychain and the
/// file location and use cheap key-derivation settings.
fn chain(
    env: Box<dyn SecretStore>,
    keychain: Box<dyn SecretStore>,
    file: Option<PathBuf>,
    passphrase: Option<SecretString>,
    params: KdfParams,
) -> LayeredStore {
    let mut layers = vec![env, keychain];
    if let Some(path) = file {
        let unlocked = passphrase.filter(|p| !p.is_empty());
        layers.push(match unlocked {
            Some(p) => Box::new(EncryptedFileStore::with_params(path, p, params)),
            None => Box::new(LockedFile { path }),
        });
    }
    LayeredStore::new(layers)
}

/// The secret file's place in the chain when no passphrase was given.
struct LockedFile {
    path: PathBuf,
}

impl LockedFile {
    /// The error for any use that needs the file's contents. Never carries a secret.
    fn refuse(&self) -> AiError {
        if self.path.exists() {
            AiError::new(
                ErrorKind::Auth,
                "the saved keys file is locked; enter its passphrase to use it",
            )
        } else {
            AiError::new(
                ErrorKind::Auth,
                "this computer has no system keychain, so choose a passphrase to keep keys in an \
                 encrypted file",
            )
        }
    }
}

impl SecretStore for LockedFile {
    /// Same name as the unlocked file store, so diagnostics read the same either way.
    fn name(&self) -> &'static str {
        "encrypted file"
    }

    /// No file means no key here; a file we cannot open is an error, not "absent".
    fn get(&self, _key: &SecretRef) -> Result<Option<SecretString>, AiError> {
        if self.path.exists() {
            return Err(self.refuse());
        }
        Ok(None)
    }

    /// Always refuses, saying what the person must do (unlock the file, or choose a passphrase).
    fn set(&self, _key: &SecretRef, _value: &SecretString) -> Result<(), AiError> {
        Err(self.refuse())
    }

    /// Nothing to remove without a file; a locked file cannot be changed.
    fn delete(&self, _key: &SecretRef) -> Result<(), AiError> {
        if self.path.exists() {
            return Err(self.refuse());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "standard_tests.rs"]
mod tests;
