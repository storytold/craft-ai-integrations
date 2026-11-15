//! Tests for the standard chain: the passphrase rule, and the file layer with and without a
//! passphrase, beside a working or missing keychain.

use std::path::Path;
use std::sync::Arc;

use super::*;
use crate::secrets::MemoryStore;

/// A secret reference from a test name.
fn key(s: &str) -> SecretRef {
    SecretRef::new(s).unwrap()
}

/// Tiny key-derivation settings so the encrypted-file tests stay fast.
fn cheap() -> KdfParams {
    KdfParams::new(64, 1, 1).unwrap()
}

/// An environment layer that never holds a key.
fn no_env() -> Box<dyn SecretStore> {
    Box::new(EnvStore::with_lookup(|_| None))
}

/// A keychain that is not there, as on a bare Linux session.
struct NoKeychain;

impl SecretStore for NoKeychain {
    /// Names itself as the keychain layer.
    fn name(&self) -> &'static str {
        "keychain"
    }
    /// Unavailable, so the chain moves on.
    fn get(&self, _: &SecretRef) -> Result<Option<SecretString>, AiError> {
        Err(AiError::unsupported("no OS keychain is available"))
    }
    /// Unavailable, so the chain moves on.
    fn set(&self, _: &SecretRef, _: &SecretString) -> Result<(), AiError> {
        Err(AiError::unsupported("no OS keychain is available"))
    }
    /// Unavailable, so the chain moves on.
    fn delete(&self, _: &SecretRef) -> Result<(), AiError> {
        Err(AiError::unsupported("no OS keychain is available"))
    }
}

/// A working keychain the test can look inside afterwards.
struct Keychain(Arc<MemoryStore>);

impl SecretStore for Keychain {
    /// Names itself as the keychain layer.
    fn name(&self) -> &'static str {
        "keychain"
    }
    /// Reads from the shared map.
    fn get(&self, k: &SecretRef) -> Result<Option<SecretString>, AiError> {
        self.0.get(k)
    }
    /// Writes to the shared map.
    fn set(&self, k: &SecretRef, v: &SecretString) -> Result<(), AiError> {
        self.0.set(k, v)
    }
    /// Removes from the shared map.
    fn delete(&self, k: &SecretRef) -> Result<(), AiError> {
        self.0.delete(k)
    }
}

/// Where the secret file goes inside a test's temporary folder.
fn file_in(dir: &Path) -> PathBuf {
    dir.join("secrets.enc.json")
}

/// The standard chain on a machine with no keychain, with the file in `dir`.
fn no_keychain(dir: &Path, passphrase: Option<&str>) -> LayeredStore {
    chain(
        no_env(),
        Box::new(NoKeychain),
        Some(file_in(dir)),
        passphrase.map(SecretString::new),
        cheap(),
    )
}

/// The passphrase rule: create, unlock or not needed, from the keychain and the file.
#[test]
fn passphrase_is_asked_for_only_when_it_matters() {
    assert_eq!(need_for(true, Some(false)), PassphraseNeed::NotNeeded);
    assert_eq!(need_for(false, Some(false)), PassphraseNeed::Create);
    assert_eq!(need_for(false, Some(true)), PassphraseNeed::Unlock);
    // A file from an earlier keychain-less session still holds keys worth unlocking.
    assert_eq!(need_for(true, Some(true)), PassphraseNeed::Unlock);
    // With no config folder there is no file to create, so asking would achieve nothing.
    assert_eq!(need_for(false, None), PassphraseNeed::NotNeeded);
    assert_eq!(need_for(true, None), PassphraseNeed::NotNeeded);
}

/// Without a keychain or passphrase, reads are empty and a save asks for a passphrase.
#[test]
fn no_keychain_and_no_passphrase_says_what_to_do_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let s = no_keychain(dir.path(), None);
    assert!(s.get(&key("a")).unwrap().is_none());
    let e = s
        .set(&key("a"), &SecretString::new("sk-needle"))
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Auth);
    assert!(e.message.contains("passphrase"), "{}", e.message);
    assert!(!e.message.contains("sk-needle"));
    assert!(!file_in(dir.path()).exists());
    s.delete(&key("a")).unwrap();
}

/// With a passphrase, keys go to the encrypted file, never in the clear, and persist.
#[test]
fn no_keychain_with_a_passphrase_saves_to_the_encrypted_file() {
    let dir = tempfile::tempdir().unwrap();
    let s = no_keychain(dir.path(), Some("pw"));
    s.set(&key("a"), &SecretString::new("sk-needle")).unwrap();
    assert!(file_in(dir.path()).exists());
    let raw = std::fs::read_to_string(file_in(dir.path())).unwrap();
    assert!(!raw.contains("sk-needle"));
    let again = no_keychain(dir.path(), Some("pw"));
    assert_eq!(again.get(&key("a")).unwrap().unwrap().expose(), "sk-needle");
    again.delete(&key("a")).unwrap();
    assert!(again.get(&key("a")).unwrap().is_none());
}

/// An existing file with no passphrase refuses every use and is left unchanged.
#[test]
fn a_locked_file_is_reported_not_treated_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    no_keychain(dir.path(), Some("pw"))
        .set(&key("a"), &SecretString::new("v"))
        .unwrap();
    let locked = no_keychain(dir.path(), None);
    let e = locked.get(&key("a")).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Auth);
    assert!(e.message.contains("locked"), "{}", e.message);
    let e = locked.set(&key("b"), &SecretString::new("w")).unwrap_err();
    assert!(e.message.contains("locked"), "{}", e.message);
    assert!(locked.delete(&key("a")).is_err());
    // Still readable once unlocked: the refusals changed nothing.
    let open = no_keychain(dir.path(), Some("pw"));
    assert_eq!(open.get(&key("a")).unwrap().unwrap().expose(), "v");
}

/// An empty passphrase gives the locked stand-in, not a file store that errors on reads.
#[test]
fn an_empty_passphrase_counts_as_none() {
    let dir = tempfile::tempdir().unwrap();
    let s = no_keychain(dir.path(), Some(""));
    // With no file yet, a missing key still reads as absent rather than as an error.
    assert!(s.get(&key("a")).unwrap().is_none());
    let e = s.set(&key("a"), &SecretString::new("v")).unwrap_err();
    assert!(e.message.contains("choose a passphrase"), "{}", e.message);
    assert!(!file_in(dir.path()).exists());
}

/// Beside a keychain, saves and deletes use it and no secret file is created.
#[test]
fn with_a_keychain_saves_go_there_and_a_missing_file_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mem = Arc::new(MemoryStore::new());
    let s = chain(
        no_env(),
        Box::new(Keychain(mem.clone())),
        Some(file_in(dir.path())),
        None,
        cheap(),
    );
    assert!(s.get(&key("a")).unwrap().is_none());
    s.set(&key("a"), &SecretString::new("v")).unwrap();
    assert_eq!(mem.get(&key("a")).unwrap().unwrap().expose(), "v");
    assert!(!file_in(dir.path()).exists());
    s.delete(&key("a")).unwrap();
    assert!(mem.get(&key("a")).unwrap().is_none());
}

/// A key in the keychain is found even when a locked file sits below it.
#[test]
fn a_keychain_hit_is_not_hidden_by_a_locked_file() {
    let dir = tempfile::tempdir().unwrap();
    no_keychain(dir.path(), Some("pw"))
        .set(&key("old"), &SecretString::new("in-file"))
        .unwrap();
    let mem = Arc::new(MemoryStore::new());
    mem.set(&key("a"), &SecretString::new("in-keychain"))
        .unwrap();
    let s = chain(
        no_env(),
        Box::new(Keychain(mem)),
        Some(file_in(dir.path())),
        None,
        cheap(),
    );
    assert_eq!(s.get(&key("a")).unwrap().unwrap().expose(), "in-keychain");
    // A key only the locked file could hold is an error, not "no key saved".
    assert_eq!(s.get(&key("old")).unwrap_err().kind, ErrorKind::Auth);
}

/// The file layer is present exactly when there is a location for it.
#[test]
fn no_config_folder_leaves_environment_and_keychain_only() {
    let s = chain(no_env(), Box::new(NoKeychain), None, None, cheap());
    assert_eq!(s.layer_names(), vec!["environment", "keychain"]);
    let with_file = chain(
        no_env(),
        Box::new(NoKeychain),
        Some(PathBuf::from("unused")),
        None,
        cheap(),
    );
    assert_eq!(
        with_file.layer_names(),
        vec!["environment", "keychain", "encrypted file"]
    );
}

/// A path no file check can answer for: a NUL byte makes every platform refuse it.
fn unchecked_path() -> PathBuf {
    PathBuf::from("no\0such").join("secrets.enc.json")
}

/// A file check that fails counts as a file that may hold keys, never as no file.
#[test]
fn a_failed_file_check_counts_as_locked_not_missing() {
    assert!(on_disk(Err(std::io::Error::other("denied"))));
    assert!(!on_disk(Ok(false)));
    let s = chain(
        no_env(),
        Box::new(NoKeychain),
        Some(unchecked_path()),
        None,
        cheap(),
    );
    let e = s.get(&key("a")).unwrap_err();
    assert!(e.message.contains("locked"), "{}", e.message);
    assert!(s.delete(&key("a")).is_err());
}

/// The unlocked file store does not report a delete as done when it cannot check the file.
#[test]
fn an_unlocked_delete_that_cannot_check_the_file_fails() {
    let s = EncryptedFileStore::with_params(unchecked_path(), SecretString::new("pw"), cheap());
    assert!(s.delete(&key("a")).is_err());
}
