//! [`AiClient`]: what an app calls.
//!
//! One flow for every Craft app, matching the adapter checklist:
//!
//! 1. [`status`](AiClient::status): is this feature usable? Gives a plain reason when it is not
//!    (no model chosen, connection switched off, key missing), so the app can grey the feature out
//!    and point at the launcher. Every app works with no connection at all.
//! 2. [`plan`](AiClient::plan): resolve the role, check the request against the chosen model, and
//!    build the [`Disclosure`] of what would leave the machine. Nothing is sent and no key is read.
//! 3. The app shows the disclosure if [`Plan::needs_consent`], and records agreement in its
//!    [`ConsentLedger`].
//! 4. [`start`](AiClient::start): re-checks consent (so it cannot be skipped), reads the key, builds
//!    the provider and submits. The returned [`Job`] is polled and cancelled by the app's own job
//!    system.
//!
//! The key exists only inside `start`, from the moment it is read until the adapter is built. It is
//! never in a [`Plan`], a [`Job`], a status or an error.

use std::sync::Arc;

use craft_ai_core::job::Event;
use craft_ai_core::model::InputRole;
use craft_ai_core::picks::{Candidate, choose};
use craft_ai_core::role::eligible;
use craft_ai_core::{
    AiError, AppId, Backoff, Billing, Binding, Catalog, Connection, ConnectionId, ConsentLedger,
    CostEstimate, Disclosure, ErrorKind, HistoryLog, JobId, JobSnapshot, JobState, Locality,
    ModelId, Outcome, Pick, PickedBy, Provider, Request, Role, RoleId, SecretString, Usage,
};
use craft_ai_native::secrets::{SecretStore, secret_for, standard_store};
use craft_ai_native::shared::SharedStore;
use craft_ai_native::{CatalogFile, HistoryFile};

use crate::factory::ProviderFactory;
use crate::history::Recorder;
use crate::route_support::{RouteFacts, RouteQuery, RouteSupport};

#[path = "plan_cost.rs"]
mod plan_cost;
#[path = "client_run.rs"]
mod run;
#[path = "client_wait.rs"]
mod wait;

/// What a person is told when the app has no adapter for the model they chose. One sentence, so
/// status, plan and start agree and the plain-language test can pin it.
const NO_ADAPTER: &str = "This app can't run the chosen model yet. Choose another in Set up AI.";

/// Whether a role can run right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoleStatus {
    Ready(RoleReady),
    /// Not usable, with a sentence the app can show.
    Unavailable {
        reason: String,
    },
}

/// What a ready role is bound to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleReady {
    pub connection: ConnectionId,
    pub connection_label: String,
    pub model: ModelId,
    pub model_label: String,
    pub locality: Locality,
    pub billing: Billing,
}

/// One entry in a model picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub connection: ConnectionId,
    pub connection_label: String,
    pub model: ModelId,
    pub model_label: String,
    pub locality: Locality,
}

/// A model that earned a preset for a job, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickedChoice {
    pub pick: Pick,
    pub choice: Choice,
    pub by: PickedBy,
}

/// A checked request waiting for consent. Holds no secret.
#[derive(Clone, Debug)]
pub struct Plan {
    pub(crate) connection: Connection,
    pub(crate) model: ModelId,
    pub(crate) request: Request,
    pub(crate) disclosure: Disclosure,
    pub(crate) role_id: RoleId,
    pub(crate) role_label: String,
    /// Worked out at plan time from the saved model's pricing; see [`Plan::estimate`].
    pub(crate) estimate: Option<CostEstimate>,
}

/// Unix seconds now; 0 if the clock is before 1970, which makes timed agreements read as valid
/// rather than lapsed, so a broken clock never asks twice.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Plan {
    /// What would be sent where. Show this to the user.
    pub fn disclosure(&self) -> &Disclosure {
        &self.disclosure
    }

    /// True when the person must agree first: nothing on file covers this app, connection,
    /// destination, capability and kind of data, or the agreement has lapsed.
    pub fn needs_consent(&self, ledger: &ConsentLedger) -> bool {
        self.needs_consent_at(ledger, now_secs())
    }

    /// [`needs_consent`](Self::needs_consent) at a given time (unix seconds).
    pub fn needs_consent_at(&self, ledger: &ConsentLedger, now: u64) -> bool {
        ledger.needs_consent(&self.disclosure, now)
    }

    pub fn connection_label(&self) -> &str {
        &self.connection.label
    }

    /// The adapter name as stored on the connection, for adapter-specific params.
    ///
    /// The factory matches names case-insensitively. This accessor preserves the original
    /// spelling, so compare with `eq_ignore_ascii_case` (for example against `"bfl"`).
    pub fn provider(&self) -> &str {
        &self.connection.provider
    }

    /// The model the plan will run, as resolved from the binding.
    pub fn model(&self) -> &ModelId {
        &self.model
    }
}

/// A started job.
#[derive(Clone)]
pub struct Job {
    provider: Arc<dyn Provider>,
    id: JobId,
    recorder: Option<Arc<Recorder>>,
    poll_retry: Backoff,
}

impl std::fmt::Debug for Job {
    /// Shows the job id only: the provider and its key stay out of logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Job({})", self.id)
    }
}

/// Everything a finished job produced.
#[derive(Debug)]
pub struct Finished {
    pub state: JobState,
    pub events: Vec<Event>,
    pub usage: Option<Usage>,
}

impl Job {
    /// The provider's id for this job.
    pub fn id(&self) -> &JobId {
        &self.id
    }

    /// The job's state now. An error that [`AiError::is_retryable`] (a dropped connection, a
    /// rate limit) says nothing about the job itself: poll again after a pause, honouring
    /// `retry_after_secs`, as [`wait`](Self::wait) does.
    pub fn poll(&self) -> Result<JobSnapshot, AiError> {
        let snap = self.provider.poll(&self.id)?;
        if let Some(recorder) = &self.recorder {
            recorder.observe(&snap.state, snap.usage.as_ref());
        }
        Ok(snap)
    }

    /// Asks the provider to stop the job. A job that has already ended is the provider's to report.
    pub fn cancel(&self) -> Result<(), AiError> {
        self.provider.cancel(&self.id)
    }
}

/// The app's connection to the AI layer.
pub struct AiClient {
    app: AppId,
    pub(crate) shared: SharedStore,
    pub(crate) secrets: Arc<dyn SecretStore>,
    pub(crate) factory: ProviderFactory,
    history: Option<Arc<HistoryFile>>,
    keep_prompts: bool,
    catalog: Catalog,
    submit_retry: Backoff,
    poll_retry: Backoff,
    /// Set by `with_retries`, so `with_factory` hands the send rule to a later factory.
    send_rule_chosen: bool,
}

impl AiClient {
    /// A client for `app` over the given shared store and secret store, with the standard
    /// adapters, no history file, the built-in catalog and the default retry rules.
    pub fn new(app: AppId, shared: SharedStore, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            app,
            shared,
            secrets,
            factory: ProviderFactory::standard(),
            history: None,
            keep_prompts: false,
            catalog: Catalog::builtin(),
            submit_retry: Backoff::STANDARD,
            poll_retry: wait::POLL_RETRY,
            send_rule_chosen: false,
        }
    }

    /// The usual setup: the standard shared store location, and keys from the environment, then
    /// the OS keychain, then the encrypted secret file (locked, so a key kept only there is
    /// reported as locked rather than missing). Use
    /// [`standard_with_passphrase`](Self::standard_with_passphrase) when
    /// [`passphrase_need`](craft_ai_native::secrets::passphrase_need) says to ask for one.
    pub fn standard(app: AppId) -> Result<Self, AiError> {
        Self::standard_from(app, None)
    }

    /// As [`standard`](Self::standard), with the secret file unlocked by `passphrase`: the
    /// fallback for a machine with no OS keychain. The passphrase is held only in memory.
    pub fn standard_with_passphrase(app: AppId, passphrase: SecretString) -> Result<Self, AiError> {
        Self::standard_from(app, Some(passphrase))
    }

    /// Shared body of the two standard constructors.
    fn standard_from(app: AppId, passphrase: Option<SecretString>) -> Result<Self, AiError> {
        let shared = SharedStore::at_default_location()?;
        let secrets = standard_store(passphrase);
        let mut client = Self::new(app, shared, Arc::new(secrets));
        client.history = HistoryFile::at_default_location().map(Arc::new);
        Ok(client)
    }

    /// Logs every job to `file` (see [`history`](Self::history)). [`standard`](Self::standard)
    /// already does this at the default location; `new` does not.
    pub fn with_history(mut self, file: HistoryFile) -> Self {
        self.history = Some(Arc::new(file));
        self
    }

    /// Replaces the curated catalog the presets use (the built-in one is empty to start with).
    pub fn with_catalog(mut self, catalog: Catalog) -> Self {
        self.catalog = catalog;
        self
    }

    /// Uses the curated picks in the shared folder's catalog file over the built-in ones, so a
    /// newer list reaches every app without a release. A missing or damaged file leaves the
    /// built-in picks (call [`CatalogFile::load`] to show a person why).
    pub fn with_catalog_file(self, file: &CatalogFile) -> Self {
        self.with_catalog(file.load().catalog)
    }

    /// Stops logging jobs.
    pub fn without_history(mut self) -> Self {
        self.history = None;
        self
    }

    /// Whether the log keeps the prompt text (clipped). Off by default: the log then holds what
    /// was sent and where, not what was said.
    pub fn keep_prompts_in_history(mut self, keep: bool) -> Self {
        self.keep_prompts = keep;
        self
    }

    /// Every logged job from every app, newest first. Lines that cannot be read are counted in
    /// [`HistoryLog::skipped`] rather than failing the read.
    pub fn history(&self) -> Result<HistoryLog, AiError> {
        let file = self.history_file()?;
        let read = file
            .read()
            .map_err(|e| AiError::new(ErrorKind::Internal, format!("history unavailable: {e}")))?;
        Ok(craft_ai_core::history::merge(read.records, read.skipped))
    }

    /// Deletes the log for every app.
    pub fn clear_history(&self) -> Result<(), AiError> {
        self.history_file()?
            .clear()
            .map_err(|e| AiError::new(ErrorKind::Internal, format!("history unavailable: {e}")))
    }

    /// The history file, or `Unsupported` when this client keeps no history.
    fn history_file(&self) -> Result<&Arc<HistoryFile>, AiError> {
        self.history.as_ref().ok_or_else(|| {
            AiError::new(ErrorKind::Unsupported, "request history is turned off here")
        })
    }

    /// Replaces the provider factory, to add adapters. A send rule already chosen with
    /// [`with_retries`](Self::with_retries), and a free-memory reader given with
    /// [`with_free_memory`](Self::with_free_memory), are applied to it, so the order of the calls
    /// does not matter.
    pub fn with_factory(mut self, factory: ProviderFactory) -> Self {
        let free = self.factory.free_memory.take();
        self.factory = factory;
        if self.send_rule_chosen() {
            self.factory.set_send_retry(self.submit_retry());
        }
        if free.is_some() {
            self.factory.set_free_memory(free);
        }
        self
    }

    pub fn app(&self) -> &AppId {
        &self.app
    }

    pub(crate) fn has_adapter(&self, provider: &str) -> bool {
        self.factory.knows(provider)
    }

    pub(crate) fn declared_accelerators(
        &self,
        provider: &str,
    ) -> Option<Vec<craft_ai_features::GpuVendor>> {
        self.factory.accelerators(provider).map(<[_]>::to_vec)
    }

    /// Where this app's consent ledger is kept by default.
    pub fn consent_file(&self) -> Option<craft_ai_native::ConsentFile> {
        craft_ai_native::ConsentFile::at_default_location(&self.app)
    }

    /// Can `role` run now? The reason, when it cannot, is safe to show.
    pub fn status(&self, role: &Role) -> RoleStatus {
        let unavailable = |reason: String| RoleStatus::Unavailable { reason };
        let store = match self.shared.load() {
            Ok(s) => s,
            Err(e) => return unavailable(e.to_string()),
        };
        let resolved = match store.resolve(&self.app, role) {
            Ok(r) => r,
            Err(e) => return unavailable(e.to_string()),
        };
        if !self.factory.knows(&resolved.connection.provider) {
            return unavailable(NO_ADAPTER.to_owned());
        }
        // Before the key: a model this build would refuse is not ready, whatever is saved.
        if let RouteSupport::Unsupported { reason } = self.route_support(
            resolved.connection,
            &resolved.model.id,
            &role.capability,
            &role.inputs,
        ) {
            return unavailable(reason);
        }
        if let Err(e) = secret_for(&*self.secrets, resolved.connection) {
            return unavailable(e.message);
        }
        // A model run from a file on this computer is not ready once the file has gone.
        if let Some(file) = resolved.model.local_file.as_deref()
            && !std::path::Path::new(file).is_file()
        {
            return unavailable(
                "The file for the model you chose is no longer there. Open Set up AI to fix it."
                    .to_owned(),
            );
        }
        RoleStatus::Ready(RoleReady {
            connection: resolved.connection.id.clone(),
            connection_label: resolved.connection.label.clone(),
            model: resolved.model.id.clone(),
            model_label: resolved.model.label.clone(),
            locality: resolved.connection.locality,
            billing: resolved.connection.billing,
        })
    }

    /// What this build's adapter says about a tool's path, asked as status and the picker ask it:
    /// one output, no streaming and no options. Reads no key and contacts nothing.
    pub(crate) fn route_support(
        &self,
        connection: &Connection,
        model: &ModelId,
        capability: &craft_ai_core::CapabilityId,
        inputs: &[InputRole],
    ) -> RouteSupport {
        self.factory.route_support(&RouteQuery {
            connection,
            model,
            capability,
            inputs,
            outputs: 1,
            stream: false,
            has_options: false,
        })
    }

    /// The saved choice for `role`, what this build can do with it, and what has really worked, as
    /// separate facts. Reads no key and contacts nothing.
    pub fn route_facts(&self, role: &Role) -> RouteFacts {
        let not_configured = |why: String| RouteFacts {
            configured: Err(why),
            executable: RouteSupport::Untested,
            last_success_ms: None,
        };
        let store = match self.shared.load() {
            Ok(store) => store,
            Err(e) => return not_configured(e.to_string()),
        };
        let resolved = match store.resolve(&self.app, role) {
            Ok(resolved) => resolved,
            Err(e) => return not_configured(e.to_string()),
        };
        let executable = if self.factory.knows(&resolved.connection.provider) {
            self.route_support(
                resolved.connection,
                &resolved.model.id,
                &role.capability,
                &role.inputs,
            )
        } else {
            RouteSupport::Unsupported {
                reason: NO_ADAPTER.to_owned(),
            }
        };
        RouteFacts {
            configured: Ok(()),
            executable,
            last_success_ms: self.last_success(resolved.connection, &resolved.model.id),
        }
    }

    /// When this connection and model last finished a real job at the connection's current address,
    /// from request history if it is on. Editing the address keeps the connection's id, so a job
    /// sent to the old address does not count.
    fn last_success(&self, connection: &Connection, model: &ModelId) -> Option<u64> {
        let destination = Disclosure::destination_for(connection);
        self.history()
            .ok()?
            .entries
            .iter()
            .filter(|e| e.start.connection == connection.id.as_str())
            .filter(|e| e.start.model == model.as_str())
            .filter(|e| e.start.destination == destination)
            .filter_map(|e| e.end.as_ref())
            .filter(|end| end.outcome == Outcome::Done)
            .map(|end| end.at_ms)
            .max()
    }

    /// Whether this app cannot run this model for `role`, so it must not be offered here: this
    /// build has no adapter for the connection, or the adapter would refuse it.
    fn known_unsupported(&self, role: &Role, connection: &Connection, model: &ModelId) -> bool {
        !self.factory.knows(&connection.provider)
            || matches!(
                self.route_support(connection, model, &role.capability, &role.inputs),
                RouteSupport::Unsupported { .. }
            )
    }

    /// Every model that could fill `role`, for a picker. One the adapter would refuse is left out.
    pub fn choices(&self, role: &Role) -> Result<Vec<Choice>, AiError> {
        let store = self.shared.load()?;
        store
            .check_tool(role)
            .map_err(|e| AiError::invalid(e.to_string()))?;
        Ok(eligible(role, &store.connections)
            .into_iter()
            .filter(|r| !self.known_unsupported(role, r.connection, &r.model.id))
            .map(|r| Choice {
                connection: r.connection.id.clone(),
                connection_label: r.connection.label.clone(),
                model: r.model.id.clone(),
                model_label: r.model.label.clone(),
                locality: r.connection.locality,
            })
            .collect())
    }

    /// The presets (recommended, lowest cost, newest and so on) each model earns for `role`, in
    /// picker order. A preset nobody earns is left out: a curated pick the connections do not offer
    /// is skipped, and cheapest or newest need the provider to have reported a price or a date.
    /// Nothing is bound; the app offers these and binds only when the person agrees.
    pub fn picks(&self, role: &Role) -> Result<Vec<PickedChoice>, AiError> {
        let store = self.shared.load()?;
        store
            .check_tool(role)
            .map_err(|e| AiError::invalid(e.to_string()))?;
        let mut eligible = eligible(role, &store.connections);
        eligible.retain(|r| !self.known_unsupported(role, r.connection, &r.model.id));
        let candidates: Vec<Candidate<'_>> = eligible
            .iter()
            .map(|r| Candidate {
                provider: &r.connection.provider,
                model: r.model,
                free: r.connection.billing == Billing::Free,
            })
            .collect();
        let mut out = Vec::new();
        for picked in choose(&role.capability, &candidates, &self.catalog) {
            let Some(r) = eligible.get(picked.candidate) else {
                continue;
            };
            out.push(PickedChoice {
                pick: picked.pick,
                choice: Choice {
                    connection: r.connection.id.clone(),
                    connection_label: r.connection.label.clone(),
                    model: r.model.id.clone(),
                    model_label: r.model.label.clone(),
                    locality: r.connection.locality,
                },
                by: picked.by,
            });
        }
        Ok(out)
    }

    /// For each of `roles` that cannot run yet, the model the catalog recommends. This is what to
    /// offer after a connection tests well ("use the recommended models for N features?"); it binds
    /// nothing, so a model is never chosen for the person without their yes.
    pub fn recommended(&self, roles: &[Role]) -> Result<Vec<(RoleId, Choice)>, AiError> {
        let mut out = Vec::new();
        for role in roles {
            if matches!(self.status(role), RoleStatus::Ready(_)) {
                continue;
            }
            if let Some(p) = self
                .picks(role)?
                .into_iter()
                .find(|p| p.pick == Pick::Recommended)
            {
                out.push((role.id.clone(), p.choice));
            }
        }
        Ok(out)
    }

    /// Sets this app's own choice for `role`, overriding the launcher default. Refused, and nothing
    /// written, if the connection or model cannot fill the role.
    pub fn bind(
        &self,
        role: &Role,
        connection: &ConnectionId,
        model: &ModelId,
    ) -> Result<(), AiError> {
        let binding = Binding {
            role: role.id.clone(),
            connection: connection.clone(),
            model: model.clone(),
        };
        self.shared.try_update(|store| {
            store
                .check_tool(role)
                .map_err(|e| AiError::invalid(e.to_string()))?;
            craft_ai_core::role::resolve(
                role,
                &[std::slice::from_ref(&binding)],
                &store.connections,
            )
            .map_err(|e| AiError::invalid(e.to_string()))
            .and_then(|resolved| {
                // This app's own choice must be one it can run, unlike the suite default.
                if !self.factory.knows(&resolved.connection.provider) {
                    return Err(AiError::unsupported(NO_ADAPTER));
                }
                match self.route_support(resolved.connection, model, &role.capability, &role.inputs)
                {
                    RouteSupport::Unsupported { reason } => Err(AiError::unsupported(reason)),
                    _ => Ok(()),
                }
            })?;
            set_override(store, &self.app, binding);
            Ok::<(), AiError>(())
        })??;
        Ok(())
    }

    /// Removes this app's own choice, so the launcher default applies again.
    pub fn clear_binding(&self, role: &RoleId) -> Result<(), AiError> {
        // With no choice to remove, nothing is written: no file is created and no bytes change.
        // `Err(())` from the closure only means there was nothing to clear.
        let _ = self.shared.try_update(|file| {
            let settings = file.apps.get_mut(&self.app).ok_or(())?;
            let before = settings.overrides.len();
            settings.overrides.retain(|b| &b.role != role);
            if settings.overrides.len() == before {
                return Err(());
            }
            Ok(())
        })?;
        Ok(())
    }
}

impl std::fmt::Debug for AiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AiClient({})", self.app)
    }
}

fn set_override(store: &mut craft_ai_core::StoreFile, app: &AppId, binding: Binding) {
    let settings = store.apps.entry(app.clone()).or_default();
    settings.overrides.retain(|b| b.role != binding.role);
    settings.overrides.push(binding);
}
