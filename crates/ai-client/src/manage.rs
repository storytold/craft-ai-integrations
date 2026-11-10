//! Adding, editing, removing and testing connections: what the launcher's connections screen (and
//! the small fallback form inside an app) calls.
//!
//! It is the one writer of the shared store and of the keys that go with it, so the two cannot
//! drift apart. The rules a person types against live in
//! [`ConnectionDraft`](craft_ai_core::ConnectionDraft); this adds the parts that touch disk and the
//! keychain, in an order that cannot lose a key:
//!
//! - **Save** writes a new key into a slot of its own with no lock held, then points the
//!   connection at it under the file lock, then deletes the slot it replaced. No key is ever
//!   overwritten, so a failed save only deletes the slot it just wrote, two saves at once can
//!   never share or delete each other's key, and the lock is never held across the keychain.
//! - **Delete** removes the connection and every default or app choice that pointed at it in one
//!   locked write, then its key (unless another connection still uses that locator).
//! - **Test** asks the server for its models with short timeouts, without saving anything.
//!
//! Every call blocks, so a screen runs it on a worker thread. Nothing here puts a key in an error
//! message, a log or a file other than the secret store.

use std::sync::Arc;
use std::time::{Duration, Instant};

use craft_ai_core::{
    AiError, Auth, Connection, ConnectionDraft, ConnectionId, DraftError, ErrorKind, KeyChange,
    ModelInfo, SecretRef, SecretString, StoreFile,
};
use craft_ai_native::secrets::{SecretStore, secret_for, standard_store};
use craft_ai_native::shared::SharedStore;

use crate::assignments::{
    UnavailableChoice, refreshed_models, unavailable_choices, without_withdrawn,
};
use crate::factory::ProviderFactory;

mod probe;
mod slot;
pub use probe::{
    Confirmed, MAX_DECLARED, ModelChoice, ProbeReport, ProbedModel, Unconfirmed, declare_models,
};

/// Whether a connection's key is where it should be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyState {
    /// The connection needs no key.
    NotNeeded,
    Saved,
    /// It needs a key and none is saved.
    Missing,
    /// The store could not be read (a locked keychain, a damaged file).
    Unreadable,
}

/// One saved connection as a screen shows it.
#[derive(Clone, Debug)]
pub struct ConnectionView {
    pub connection: Connection,
    pub key: KeyState,
    /// Default or app choices that use it, so a delete can warn.
    pub bindings: usize,
}

/// Why a save did not happen.
#[derive(Debug)]
pub enum SaveError {
    /// The form has a problem the person can fix.
    Invalid(DraftError),
    /// The disk or keychain refused. The message is safe to show.
    Failed(AiError),
}

impl std::fmt::Display for SaveError {
    /// The message of the draft or store error, which is safe to show.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(e) => e.fmt(f),
            Self::Failed(e) => f.write_str(&e.message),
        }
    }
}

impl std::error::Error for SaveError {}

/// A completed save.
#[derive(Clone, Debug)]
pub struct Saved {
    pub id: ConnectionId,
    /// Something minor went wrong after the save (an old key could not be removed).
    pub warning: Option<String>,
    /// Choices this connection can no longer serve after the save: the model is gone, or still
    /// listed without the ability or an input the tool needs. They are kept, in case it comes back.
    pub unavailable_choices: Vec<UnavailableChoice>,
}

/// A completed delete.
#[derive(Clone, Debug)]
pub struct Removed {
    /// Default and per-app choices that pointed at it and were removed with it.
    pub bindings_removed: usize,
    /// `false` if the saved key could not be removed; the connection is gone either way.
    pub key_removed: bool,
}

/// What a test found.
#[derive(Clone, Debug)]
pub struct TestReport {
    /// The models the server offers. Empty for a custom connection, whose models are only what a
    /// person declares (see `probe`).
    pub models: Vec<ModelInfo>,
    pub elapsed: Duration,
    /// For a custom connection: what the test confirmed and could not confirm.
    pub probe: Option<ProbeReport>,
    /// Choices the connection can no longer serve once a remembered refresh is saved: the model is
    /// gone, or still listed without the ability or an input the tool needs. They are kept, in case
    /// it comes back.
    pub unavailable_choices: Vec<UnavailableChoice>,
}

/// Reads and writes the user's connections.
#[derive(Clone)]
pub struct ConnectionManager {
    shared: SharedStore,
    secrets: Arc<dyn SecretStore>,
    probe: ProviderFactory,
}

impl ConnectionManager {
    /// A manager over the given shared store and secret store, testing with the probing adapters.
    pub fn new(shared: SharedStore, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            shared,
            secrets,
            probe: ProviderFactory::probing(),
        }
    }

    /// The usual setup: the standard store location, keys in the environment (read only), then the
    /// OS keychain, then the encrypted secret file, locked. On a machine with no keychain a save
    /// is refused with a request for a passphrase; use
    /// [`standard_with_passphrase`](Self::standard_with_passphrase) once the person gives one.
    pub fn standard() -> Result<Self, AiError> {
        Self::standard_from(None)
    }

    /// As [`standard`](Self::standard), with the secret file unlocked by `passphrase`, so keys
    /// can be saved where there is no OS keychain. The passphrase is held only in memory.
    pub fn standard_with_passphrase(passphrase: SecretString) -> Result<Self, AiError> {
        Self::standard_from(Some(passphrase))
    }

    /// Shared body of the two standard constructors.
    fn standard_from(passphrase: Option<SecretString>) -> Result<Self, AiError> {
        let shared = SharedStore::at_default_location()?;
        Ok(Self::new(shared, Arc::new(standard_store(passphrase))))
    }

    /// Replaces the factory used by the Test button, to add adapters.
    pub fn with_probe_factory(mut self, factory: ProviderFactory) -> Self {
        self.probe = factory;
        self
    }

    pub fn store_path(&self) -> &std::path::Path {
        self.shared.path()
    }

    fn load(&self) -> Result<StoreFile, AiError> {
        Ok(self.shared.load()?)
    }

    /// Every connection with the state of its key.
    pub fn views(&self) -> Result<Vec<ConnectionView>, AiError> {
        let store = self.load()?;
        Ok(store
            .connections
            .iter()
            .map(|c| ConnectionView {
                key: self.key_state(c),
                bindings: count_bindings(&store, &c.id),
                connection: c.clone(),
            })
            .collect())
    }

    fn key_state(&self, c: &Connection) -> KeyState {
        let Some(key) = c.auth.secret_ref() else {
            return KeyState::NotNeeded;
        };
        match self.secrets.get(key) {
            Ok(Some(s)) if !s.is_empty() => KeyState::Saved,
            Ok(_) => KeyState::Missing,
            Err(_) => KeyState::Unreadable,
        }
    }

    /// Adds a connection or saves changes to one.
    ///
    /// A new key goes into a slot of its own, with the file lock not held (a keychain can wait on
    /// a prompt or a daemon, and every other app waits on that lock). Under the lock the file
    /// only ever swaps to a slot that already holds its key, so a connection never points at a
    /// missing key. A key is never overwritten: if the file cannot be written, the slot just
    /// written is deleted and everything else is as it was; once it is written, the slot the
    /// connection used to point at is deleted. A crash between the two writes leaves one unused
    /// slot and nothing else.
    pub fn save(&self, draft: &ConnectionDraft) -> Result<Saved, SaveError> {
        let current = self.load().map_err(SaveError::Failed)?;
        let prepared = draft
            .prepare(&current.connections)
            .map_err(SaveError::Invalid)?;

        // 1. A typed key, in a slot nobody else can be using.
        let fresh = match &prepared.key {
            KeyChange::Set(key) => Some(self.store_new_key(&prepared.connection.id, key)?),
            _ => None,
        };

        // 2. The file, re-checked as it is now. Only file work happens under the lock.
        let result = self.shared.try_update(
            |file| -> Result<(ConnectionId, Option<SecretRef>, Vec<UnavailableChoice>), SaveError> {
                let mut again = draft
                    .prepare(&file.connections)
                    .map_err(SaveError::Invalid)?;
                if let (Some(slot), Auth::ApiKey { secret }) = (&fresh, &mut again.connection.auth)
                {
                    *secret = slot.clone();
                }
                let id = again.connection.id.clone();
                let new_ref = again.connection.auth.secret_ref().cloned();
                let old_ref = match file.connections.iter_mut().find(|c| c.id == id) {
                    Some(slot) => {
                        // The form's model list may be older than the file (another app assigned a
                        // model since it opened) or a raw test answer. What a person declared for a
                        // tool is not the form's to drop while the connection still points at the
                        // same service, or a choice would stay selected with nothing to resolve.
                        if slot.provider == again.connection.provider
                            && slot.endpoint == again.connection.endpoint
                        {
                            let listed = std::mem::take(&mut again.connection.models);
                            let listed = without_withdrawn(listed, &slot.models);
                            let mut models = refreshed_models(&slot.models, listed);
                            // A model listed after the form opened is news from elsewhere, not one
                            // the form removed.
                            for later in &slot.models {
                                if !draft.opened_with().contains(&later.id)
                                    && models.iter().all(|m| m.id != later.id)
                                {
                                    models.push(later.clone());
                                }
                            }
                            again.connection.models = models;
                        }
                        let old = slot.auth.secret_ref().cloned();
                        *slot = again.connection;
                        old
                    }
                    None => {
                        file.connections.push(again.connection);
                        None
                    }
                };
                let retired = old_ref.filter(|r| {
                    Some(r) != new_ref.as_ref()
                        && !file
                            .connections
                            .iter()
                            .any(|c| c.auth.secret_ref() == Some(r))
                });
                let unavailable = unavailable_choices(file, &id);
                Ok((id, retired, unavailable))
            },
        );
        let outcome = match result {
            Ok(done) => done,
            Err(e) => Err(SaveError::Failed(e.into())),
        };

        // 3. The file did not change, so the slot just written is unused: drop it. It is ours
        // alone, so nothing else can be affected.
        let (id, retired, unavailable_choices) = match outcome {
            Ok(done) => done,
            Err(e) => {
                if let Some(slot) = &fresh {
                    let _ = self.secrets.delete(slot);
                }
                return Err(e);
            }
        };

        // 4. The file now points at the new key. A key the connection no longer uses can go.
        let warning = retired
            .filter(|old| self.secrets.delete(old).is_err())
            .map(|_| "The old key could not be removed from the keychain.".to_owned());
        Ok(Saved {
            id,
            warning,
            unavailable_choices,
        })
    }

    /// Writes `key` to a slot no other save can be using and returns where it went.
    fn store_new_key(&self, id: &ConnectionId, key: &SecretString) -> Result<SecretRef, SaveError> {
        let mut slot = slot::fresh(id).map_err(SaveError::Failed)?;
        for _ in 0..4 {
            if !matches!(self.secrets.get(&slot), Ok(Some(_))) {
                break;
            }
            slot = slot::fresh(id).map_err(SaveError::Failed)?;
        }
        if let Err(e) = self.secrets.set(&slot, key) {
            let _ = self.secrets.delete(&slot);
            // A credentials refusal (the secret file needs a passphrase, or is locked; the
            // keychain denied access) says what the person must do; store errors never carry a
            // secret, so it is safe to show. Anything else gets the general advice.
            let shown = if e.kind == ErrorKind::Auth {
                e
            } else {
                key_not_saved(&slot)
            };
            return Err(SaveError::Failed(shown));
        }
        Ok(slot)
    }

    /// Turns a connection on or off without losing it.
    pub fn set_enabled(&self, id: &ConnectionId, enabled: bool) -> Result<(), AiError> {
        // An unknown id writes nothing, so it never creates a file or rewrites one.
        self.shared.try_update(|saved| {
            saved
                .connections
                .iter_mut()
                .find(|c| &c.id == id)
                .map(|c| c.enabled = enabled)
                .ok_or_else(|| not_found(id))
        })?
    }

    /// Removes a connection, the choices that used it, and its key.
    pub fn delete(&self, id: &ConnectionId) -> Result<Removed, AiError> {
        // An unknown id writes nothing, so the file's bytes stay as they were.
        let (gone, bindings_removed, key_still_used) = self.shared.try_update(|saved| {
            let pos = saved
                .connections
                .iter()
                .position(|c| &c.id == id)
                .ok_or_else(|| not_found(id))?;
            let gone = saved.connections.remove(pos);
            let before = count_bindings(saved, id);
            saved.defaults.retain(|b| &b.connection != id);
            for app in saved.apps.values_mut() {
                app.overrides.retain(|b| &b.connection != id);
            }
            let key_still_used = gone.auth.secret_ref().is_some_and(|r| {
                saved
                    .connections
                    .iter()
                    .any(|c| c.auth.secret_ref() == Some(r))
            });
            Ok::<_, AiError>((gone, before, key_still_used))
        })??;
        let key_removed = match gone.auth.secret_ref() {
            Some(r) if !key_still_used => self.secrets.delete(r).is_ok(),
            _ => true,
        };
        Ok(Removed {
            bindings_removed,
            key_removed,
        })
    }

    /// Asks the server in `draft` for its models without saving anything, so a key can be tried
    /// before it is kept.
    pub fn test_draft(&self, draft: &ConnectionDraft) -> Result<TestReport, AiError> {
        let current = self.load()?;
        let prepared = draft
            .prepare(&current.connections)
            .map_err(|e| AiError::invalid(e.message))?;
        let secret = match (&prepared.key, prepared.connection.auth.secret_ref()) {
            (KeyChange::Set(key), _) => Some(key.clone()),
            (KeyChange::Keep, Some(r)) => self.secrets.get(r)?.filter(|s| !s.is_empty()),
            _ => None,
        };
        self.ask(prepared.connection, secret)
    }

    /// Tests a saved connection. With `remember`, the models found replace the saved list, so apps
    /// offer what the server really has. A remembered answer is refused if the saved connection
    /// changed or was removed while the test ran; the newer settings are kept.
    pub fn test_saved(&self, id: &ConnectionId, remember: bool) -> Result<TestReport, AiError> {
        let current = self.load()?;
        let connection = current
            .connections
            .iter()
            .find(|c| &c.id == id)
            .cloned()
            .ok_or_else(|| not_found(id))?;
        let secret = secret_for(self.secrets.as_ref(), &connection)?;
        let mut report = self.ask(connection.clone(), secret)?;
        // Only a list the server reported itself is remembered; a custom connection's models
        // are the ones a person declared, and a test never replaces them with guesses.
        if remember && report.probe.is_none() {
            let models = report.models.clone();
            report.unavailable_choices = self.shared.try_update(|file| {
                let saved = file
                    .connections
                    .iter_mut()
                    .find(|c| &c.id == id)
                    .ok_or_else(|| not_found(id))?;
                // Compare and write under the same lock. The probe ran without that lock, so
                // another app could have changed any part of the connection in the meantime.
                if *saved != connection {
                    return Err(AiError::invalid(
                        "the connection changed during its test; test it again",
                    ));
                }
                // What a person declared on top of the server's list is not the server's to remove.
                saved.models = refreshed_models(&saved.models, models);
                Ok(unavailable_choices(file, id))
            })??;
        }
        Ok(report)
    }

    /// Asks the server for its models, or runs the custom-connection probe, with nothing saved.
    /// For a listed service the saved list is cleared first, so the answer comes from the server.
    fn ask(
        &self,
        mut connection: Connection,
        secret: Option<craft_ai_core::SecretString>,
    ) -> Result<TestReport, AiError> {
        if connection.provider == probe::CUSTOM_PROVIDER {
            let start = Instant::now();
            let probe = probe::run(&connection, secret)?;
            return Ok(TestReport {
                models: Vec::new(),
                elapsed: start.elapsed(),
                probe: Some(probe),
                unavailable_choices: Vec::new(),
            });
        }
        // Saved models would be answered from the list, not the server.
        connection.models.clear();
        let provider = self.probe.build(&connection, secret)?;
        let start = Instant::now();
        let models = provider.models()?;
        Ok(TestReport {
            models,
            elapsed: start.elapsed(),
            probe: None,
            unavailable_choices: Vec::new(),
        })
    }
}

impl std::fmt::Debug for ConnectionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ConnectionManager({})", self.shared.path().display())
    }
}

fn count_bindings(store: &StoreFile, id: &ConnectionId) -> usize {
    let defaults = store
        .defaults
        .iter()
        .filter(|b| &b.connection == id)
        .count();
    let overrides: usize = store
        .apps
        .values()
        .map(|a| a.overrides.iter().filter(|b| &b.connection == id).count())
        .sum();
    defaults + overrides
}

fn not_found(id: &ConnectionId) -> AiError {
    AiError::new(
        ErrorKind::InvalidRequest,
        format!("there is no connection \"{id}\" (it may have been removed elsewhere)"),
    )
}

/// What to tell someone whose keychain refused a key. The cause is not shown (it can name system
/// internals), but the way out is.
fn key_not_saved(key: &SecretRef) -> AiError {
    let var = craft_ai_native::secrets::EnvStore::var_name(key);
    AiError::new(
        ErrorKind::Internal,
        format!(
            "The key could not be saved in this computer's keychain. On a computer without one, \
             set the environment variable {var} to the key instead."
        ),
    )
}
