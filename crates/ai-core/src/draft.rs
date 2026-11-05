//! The form behind "add a connection": editable text in, a checked [`Connection`] out.
//!
//! The launcher's connections screen and the small fallback form inside an app both edit a
//! [`ConnectionDraft`], so the rules live once and the two screens cannot disagree:
//!
//! - **Where it goes decides whether it leaves the computer.** The locality is derived from the
//!   address (loopback means local), never chosen, so a remote service cannot be mislabelled
//!   local to skip the consent prompt.
//! - **A key is checked before it is kept.** One line of visible ASCII, no spaces. A pasted
//!   newline would otherwise reach an HTTP header.
//! - **Editing keeps what was not touched.** Leaving the key box empty keeps the saved key; the
//!   models saved with a connection are dropped when the address changes, because they belong to
//!   the old one. Models found by a test at the new address are kept. A late answer from another
//!   address is dropped rather than credited to the address now in the form.
//! - **Messages are for people.** Every [`DraftError`] names the field and says what to do, and
//!   never contains the key.
//!
//! Nothing here does I/O, so it compiles for wasm.

use std::fmt;

use crate::connection::{Auth, Billing, Connection, Locality};
use crate::endpoint::Endpoint;
use crate::ids::{ConnectionId, SecretRef, validate_id};
use crate::model::ModelInfo;
use crate::secret::SecretString;

/// Longest label, in characters.
pub const MAX_LABEL_CHARS: usize = 80;
/// Longest address, in bytes.
pub const MAX_ENDPOINT_BYTES: usize = 2048;
/// Longest key, in bytes.
pub const MAX_KEY_BYTES: usize = 4096;

/// A starting point for the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    /// Stable name, for tests and analytics-free bookkeeping.
    pub key: &'static str,
    /// What the button says.
    pub name: &'static str,
    pub provider: &'static str,
    /// Empty for "type your own".
    pub endpoint: &'static str,
    pub billing: Billing,
    pub needs_key: bool,
    /// One line about it, shown under the name.
    pub note: &'static str,
}

pub use presets::PRESETS;
use presets::preset_models;

mod presets;

/// Which box a problem belongs to, so a form can mark it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Label,
    Provider,
    Address,
    Key,
    /// The name of the header that carries the key.
    KeyHeader,
}

/// Why a draft was refused. The message is safe to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftError {
    pub field: Field,
    pub message: String,
}

impl DraftError {
    fn new(field: Field, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }
}

impl fmt::Display for DraftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DraftError {}

/// What to do with the stored key.
#[derive(Clone, Debug)]
pub enum KeyChange {
    /// Leave whatever is saved.
    Keep,
    Set(SecretString),
    /// The connection no longer uses a key; remove the saved one.
    Clear,
}

/// A checked connection and what to do with its key.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub connection: Connection,
    pub key: KeyChange,
}

/// The form's state.
#[derive(Clone)]
pub struct ConnectionDraft {
    /// `Some` when editing an existing connection.
    pub editing: Option<ConnectionId>,
    pub label: String,
    pub provider: String,
    pub endpoint: String,
    pub billing: Billing,
    pub uses_key: bool,
    /// Text typed into the key box. Empty while editing means "keep the saved key". It is a plain
    /// string only because a text box needs one; [`Debug`] never prints it and [`prepare`]
    /// (Self::prepare) turns it into a [`SecretString`].
    pub key: String,
    /// Header that carries the key; empty sends it as `Authorization: Bearer`.
    pub key_header: String,
    pub enabled: bool,
    /// Models carried onto the saved connection. A new custom draft may declare models by hand;
    /// models found by a test must use [`set_models`](Self::set_models) to record their address.
    pub models: Vec<ModelInfo>,
    /// Editing: the saved key's locator, and whether a key was saved at all.
    secret: Option<SecretRef>,
    /// The address `models` were loaded, preset or last set for.
    models_for: Option<String>,
}

impl Default for ConnectionDraft {
    fn default() -> Self {
        Self {
            editing: None,
            label: String::new(),
            provider: "openai-compatible".to_owned(),
            endpoint: String::new(),
            billing: Billing::Unknown,
            uses_key: true,
            key: String::new(),
            key_header: String::new(),
            enabled: true,
            models: Vec::new(),
            secret: None,
            models_for: None,
        }
    }
}

impl fmt::Debug for ConnectionDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionDraft")
            .field("editing", &self.editing)
            .field("label", &self.label)
            .field("provider", &self.provider)
            .field("endpoint", &self.endpoint)
            .field("uses_key", &self.uses_key)
            .field("key_header", &self.key_header)
            .field("key", &format_args!("<{} chars>", self.key.chars().count()))
            .finish_non_exhaustive()
    }
}

impl ConnectionDraft {
    /// A new connection started from a preset.
    pub fn from_preset(preset: &Preset) -> Self {
        let models = preset_models(preset);
        Self {
            label: preset.name.to_owned(),
            provider: preset.provider.to_owned(),
            endpoint: preset.endpoint.to_owned(),
            billing: preset.billing,
            uses_key: preset.needs_key,
            models_for: (!models.is_empty()).then(|| preset.endpoint.trim().to_owned()),
            models,
            ..Self::default()
        }
    }

    /// Edits a saved connection. The key box starts empty.
    pub fn from_connection(c: &Connection) -> Self {
        Self {
            editing: Some(c.id.clone()),
            label: c.label.clone(),
            provider: c.provider.clone(),
            endpoint: c.endpoint.clone().unwrap_or_default(),
            billing: c.billing,
            uses_key: c.auth.secret_ref().is_some(),
            key: String::new(),
            key_header: c.key_header.clone().unwrap_or_default(),
            enabled: c.enabled,
            models: c.models.clone(),
            secret: c.auth.secret_ref().cloned(),
            models_for: c.endpoint.as_deref().map(|e| e.trim().to_owned()),
        }
    }

    /// Records models found by a test at `address`, the field value when the test started.
    /// An answer for an address no longer in the form is refused without changing its models.
    /// The caller must not substitute the current field value for the tested address.
    #[must_use]
    pub fn set_models(&mut self, address: &str, models: Vec<ModelInfo>) -> bool {
        if address.trim() != self.endpoint.trim() {
            return false;
        }
        self.models = models;
        self.models_for = Some(self.endpoint.trim().to_owned());
        true
    }

    /// Known provenance must match the saved address, including after repeated edits.
    /// A new custom draft can carry manually declared models without prior provenance.
    fn models_fit(&self, address: &str) -> bool {
        self.models_for
            .as_deref()
            .is_none_or(|origin| origin == address)
    }

    /// Where this would send data, as far as the address can tell. `None` while it is not a usable
    /// address, so a form can show the badge live.
    pub fn locality(&self) -> Option<Locality> {
        let endpoint = Endpoint::parse(self.endpoint.trim()).ok()?;
        Some(if endpoint.is_loopback() {
            Locality::Local
        } else {
            Locality::Remote
        })
    }

    /// Checks everything and builds the connection. `existing` is every saved connection, so new
    /// ids stay unique and an edit finds its original.
    pub fn prepare(&self, existing: &[Connection]) -> Result<Prepared, DraftError> {
        let label = self.label.trim();
        if label.is_empty() {
            return Err(DraftError::new(Field::Label, "Give the connection a name."));
        }
        if label.chars().count() > MAX_LABEL_CHARS || label.chars().any(char::is_control) {
            return Err(DraftError::new(
                Field::Label,
                format!("The name must be one line of at most {MAX_LABEL_CHARS} characters."),
            ));
        }
        let provider = self.provider.trim();
        validate_id(provider).map_err(|e| {
            DraftError::new(
                Field::Provider,
                format!("The service type is not valid ({e})."),
            )
        })?;
        let address = self.endpoint.trim();
        if address.is_empty() {
            return Err(DraftError::new(
                Field::Address,
                "Enter the address of the server.",
            ));
        }
        if address.len() > MAX_ENDPOINT_BYTES {
            return Err(DraftError::new(Field::Address, "That address is too long."));
        }
        let parsed = Endpoint::parse(address).map_err(|e| {
            DraftError::new(
                Field::Address,
                format!(
                    "That address cannot be used ({e}). Use https://, or http:// only for this computer."
                ),
            )
        })?;
        let locality = if parsed.is_loopback() {
            Locality::Local
        } else {
            Locality::Remote
        };

        let typed = self.typed_key()?;
        let key_header = self.checked_key_header()?;

        let id = match &self.editing {
            Some(id) => {
                if !existing.iter().any(|c| &c.id == id) {
                    return Err(DraftError::new(
                        Field::Label,
                        "This connection was removed elsewhere. Close the form and start again.",
                    ));
                }
                id.clone()
            }
            None => new_id(label, existing)?,
        };
        let (auth, key) = self.auth_and_key(typed, &id, existing)?;
        let connection = Connection {
            id,
            label: label.to_owned(),
            provider: provider.to_owned(),
            endpoint: Some(address.to_owned()),
            locality,
            auth,
            billing: self.billing,
            models: if self.models_fit(address) {
                self.models.clone()
            } else {
                Vec::new()
            },
            enabled: self.enabled,
            key_header,
        };
        connection
            .validate()
            .map_err(|e| DraftError::new(Field::Address, format!("{e}.")))?;
        Ok(Prepared { connection, key })
    }

    /// The typed key, checked, or `None` if the box is empty.
    fn typed_key(&self) -> Result<Option<SecretString>, DraftError> {
        let text = self.key.trim();
        if text.is_empty() {
            return Ok(None);
        }
        check_key(text).map(|()| Some(SecretString::new(text)))
    }

    fn auth_and_key(
        &self,
        typed: Option<SecretString>,
        id: &ConnectionId,
        existing: &[Connection],
    ) -> Result<(Auth, KeyChange), DraftError> {
        if !self.uses_key {
            let change = if self.secret.is_some() {
                KeyChange::Clear
            } else {
                KeyChange::Keep
            };
            return Ok((Auth::None, change));
        }
        let secret = match &self.secret {
            Some(s) => s.clone(),
            None => new_secret_ref(id, existing)?,
        };
        match typed {
            Some(key) => Ok((Auth::ApiKey { secret }, KeyChange::Set(key))),
            None if self.secret.is_some() => Ok((Auth::ApiKey { secret }, KeyChange::Keep)),
            None => Err(DraftError::new(
                Field::Key,
                "Paste the key. If this server needs none, turn off \"Uses a key\".",
            )),
        }
    }
}

/// One line of visible ASCII with no spaces: what an HTTP bearer token can be.
pub fn check_key(text: &str) -> Result<(), DraftError> {
    if text.len() > MAX_KEY_BYTES {
        return Err(DraftError::new(
            Field::Key,
            "That key is far too long to be a key.",
        ));
    }
    if !text.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(DraftError::new(
            Field::Key,
            "A key is one line of letters, digits and symbols with no spaces. Copy it again.",
        ));
    }
    Ok(())
}

use ids::{new_id, new_secret_ref};

mod custom;
mod ids;

#[cfg(test)]
mod tests;
