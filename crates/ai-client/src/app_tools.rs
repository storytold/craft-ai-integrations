//! What an app reads from the person's shared AI choices, and how it runs one.
//!
//! Every Craft app does the same first step: build one [`AiClient`] under its own app id, then ask,
//! for each tool in the suite catalogue, whether it can run now. [`AppTools`] is that step, and the
//! plan and run that follow, so an app copies no catalogue or job code and names no model, engine
//! or file.
//!
//! Invariants:
//! - Reading changes nothing: nothing is sent and the shared store is not written. A tool bound to
//!   a connection that needs a key is checked for a saved key, which asks the key store (the
//!   system keychain may ask the person); the key is dropped at once and never used or returned.
//! - [`AppTools::plan`] and [`AppTools::run`] are the one way an app runs a tool: plan (nothing is
//!   sent), the app's own consent step, then run. No app writes its own job loop.
//! - Tools people declared themselves (custom tools in the shared store) are listed after the
//!   catalogue's, so a tool someone builds is seen by every app with no change to the catalogue.
//! - A tool with no choice is `Unavailable` with a reason a person can read. It is never an error
//!   and never falls back to some other model.
//! - The answer is computed on every call, so a choice made in the launcher while the app is open
//!   is seen at once.
//! - A wait that ends without the job ending (a timeout, a poll that cannot pass) asks the job to
//!   stop. When the job cannot be stopped, the sentence returned says it may still be running;
//!   it is never reported as stopped.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use craft_ai_core::{
    AppId, ConsentLedger, Event, JobState, Locality, Output, Payload, Request, Role,
};
use craft_ai_features::builtin;

use crate::{AiClient, Job, Plan, RoleStatus};

/// How often a running job on this computer is looked at.
const STEP: Duration = Duration::from_millis(15);
/// How often a running job at an online service is looked at, so it is not asked many times a
/// second.
const REMOTE_STEP: Duration = Duration::from_secs(1);
/// Added to a sentence when a job could not be stopped.
const NOT_STOPPED: &str = "It could not be stopped, so it may still be running.";
/// Shown for a tool the catalogue and the shared store do not have.
const UNKNOWN: &str = "This tool is not part of the suite yet.";
/// The longest [`AppTools::run`] waits for a job before it gives up and cancels it.
const LIMIT: Duration = Duration::from_secs(180);

/// One catalogue tool and whether it can run in this app now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolReport {
    /// The catalogue id, such as `select-subject`.
    pub id: String,
    /// The name a person sees.
    pub title: String,
    /// Ready (with the model it is bound to) or unavailable (with the reason).
    pub status: RoleStatus,
}

/// An app's view of the shared tool choices: list them, plan a run, run it.
pub struct AppTools {
    client: AiClient,
    roles: Vec<(String, Role)>,
    running: Running,
    next_job: AtomicU64,
}

/// The jobs [`AppTools::run`] calls are waiting for, so a cancel can reach them. Each has a number
/// so a finished job removes only itself.
type Running = Arc<Mutex<Vec<(u64, Job)>>>;

/// Lets another thread stop the jobs an [`AppTools`] is waiting for.
#[derive(Clone)]
pub struct Canceller(Running);

impl Canceller {
    /// Asks every job being waited for to stop. Does nothing when none is running.
    ///
    /// # Errors
    /// Returns the sentence to show when a job could not be stopped (some online services cannot
    /// stop a job once it has started), so the app does not tell the person it stopped.
    pub fn cancel(&self) -> Result<(), String> {
        let jobs: Vec<Job> = lock(&self.0).iter().map(|(_, job)| job.clone()).collect();
        let mut refused = 0usize;
        for job in jobs {
            if !stop(&job) {
                refused = refused.saturating_add(1);
            }
        }
        match refused {
            0 => Ok(()),
            1 => Err(format!("One job did not stop. {NOT_STOPPED}")),
            n => Err(format!("{n} jobs did not stop. They may still be running.")),
        }
    }

    /// How many jobs are being waited for now, so an app can show or hide its stop button.
    pub fn waiting(&self) -> usize {
        lock(&self.0).len()
    }
}

/// Asks `job` to stop. True when it stopped or had already ended; false when the provider refused
/// and the job still reads as running (or cannot be read at all).
fn stop(job: &Job) -> bool {
    match job.cancel() {
        Ok(()) => true,
        Err(_) => job.poll().is_ok_and(|snap| snap.state.is_terminal()),
    }
}

/// The lock on the running jobs, with a poisoned lock recovered.
fn lock(running: &Running) -> MutexGuard<'_, Vec<(u64, Job)>> {
    running.lock().unwrap_or_else(PoisonError::into_inner)
}

impl AppTools {
    /// The real shared store and key stores, for the running app. `app` is the lower-case name the
    /// launcher uses (`photocraft`, `lightcraft`) and never changes: consent and per-app choices
    /// are scoped by it.
    ///
    /// # Errors
    /// Returns the sentence to show when `app` is not a valid app id or the store has no folder.
    pub fn standard(app: &str) -> Result<Self, String> {
        let app = AppId::new(app).map_err(|e| e.to_string())?;
        let client = AiClient::standard(app).map_err(|e| e.message)?;
        Ok(Self::with_client(client))
    }

    /// Any client, for tests and for apps that supply their own key stores or adapters.
    pub fn with_client(client: AiClient) -> Self {
        let roles = builtin()
            .into_iter()
            .filter_map(|feature| feature.role().map(|role| (feature.title, role)))
            .collect();
        Self {
            client,
            roles,
            running: Running::default(),
            next_job: AtomicU64::new(0),
        }
    }

    /// Every catalogue tool in catalogue order, then every custom tool in the shared store, with
    /// its status right now. Each call reads the store and may touch the key store, so call it
    /// when a menu or page opens, not on every frame.
    pub fn report(&self) -> Vec<ToolReport> {
        // A store that cannot be read lists no custom tools here; every catalogue tool's status
        // then carries the store's reason.
        let custom = self.custom_tools().unwrap_or_default();
        let builtin = self
            .roles
            .iter()
            .map(|(title, role)| (title.as_str(), role));
        let declared = custom.iter().map(|role| (role.label.as_str(), role));
        builtin
            .chain(declared)
            .map(|(title, role)| ToolReport {
                id: role.id.as_str().to_owned(),
                title: title.to_owned(),
                status: self.client.status(role),
            })
            .collect()
    }

    /// The status of one tool by catalogue id. An id the catalogue does not have is `Unavailable`,
    /// so a typo greys a tool out rather than running something else.
    pub fn status(&self, tool: &str) -> RoleStatus {
        match self.find(tool) {
            Ok(Some(role)) => self.client.status(&role),
            Ok(None) => RoleStatus::Unavailable {
                reason: UNKNOWN.to_owned(),
            },
            Err(reason) => RoleStatus::Unavailable { reason },
        }
    }

    /// The tool's role, for the `plan` and `start` calls on [`client`](Self::client). The role is
    /// owned because a custom tool is read from the shared store on each call.
    pub fn role(&self, tool: &str) -> Option<Role> {
        self.find(tool).ok().flatten()
    }

    /// The tool's role: catalogue first, then the shared store. An error is the store's reason,
    /// so a store that cannot be read is not mistaken for a tool that does not exist.
    fn find(&self, tool: &str) -> Result<Option<Role>, String> {
        if let Some((_, role)) = self.roles.iter().find(|(_, role)| role.id.as_str() == tool) {
            return Ok(Some(role.clone()));
        }
        Ok(self
            .custom_tools()?
            .into_iter()
            .find(|role| role.id.as_str() == tool))
    }

    /// The tools people declared in the shared store; a missing store has none. The store
    /// refuses a custom tool whose id is a catalogue id, so the two lists never overlap.
    ///
    /// # Errors
    /// Returns the store's reason when it cannot be read.
    fn custom_tools(&self) -> Result<Vec<Role>, String> {
        self.client
            .shared
            .load()
            .map(|store| store.custom_tools)
            .map_err(|e| e.to_string())
    }

    /// A request for `tool`: its capability and no inputs yet. `None` for an unknown tool.
    pub fn request(&self, tool: &str) -> Option<Request> {
        self.role(tool).map(|role| Request::new(role.capability))
    }

    /// Checks `request` against the model the person chose for `tool`. Sends nothing. The plan
    /// says what would leave this computer, so the app can ask for consent before [`run`](Self::run).
    ///
    /// # Errors
    /// Returns the sentence to show: the tool is unknown, nothing runnable is chosen, or the
    /// request does not fit the chosen model.
    pub fn plan(&self, tool: &str, request: Request) -> Result<Plan, String> {
        let role = self.find(tool)?.ok_or(UNKNOWN)?;
        self.client.plan(&role, request).map_err(|e| e.message)
    }

    /// Starts the planned job, waits for it, and returns what it produced. `ledger` is the
    /// person's consent; a local model needs none, so `ConsentLedger::default()` serves it.
    /// Blocks, so call it off the drawing thread. Gives up and cancels the job after three
    /// minutes; use [`run_for`](Self::run_for) for a job that takes longer, such as video.
    ///
    /// # Errors
    /// Returns the sentence to show when consent is missing, the job fails, is cancelled or times
    /// out, or it ends without finishing.
    pub fn run(&self, plan: &Plan, ledger: &ConsentLedger) -> Result<Vec<Output>, String> {
        self.run_for(plan, ledger, LIMIT)
    }

    /// [`run`](Self::run) with the longest wait chosen by the app. A job still running when
    /// `limit` passes, or whose state can no longer be read, is asked to stop; when it cannot be
    /// stopped the error says it may still be running. A job at an online service is looked at
    /// once a second, one on this computer more often.
    ///
    /// # Errors
    /// As [`run`](Self::run).
    pub fn run_for(
        &self,
        plan: &Plan,
        ledger: &ConsentLedger,
        limit: Duration,
    ) -> Result<Vec<Output>, String> {
        let job = self.client.start(plan, ledger).map_err(|e| e.message)?;
        let number = self.next_job.fetch_add(1, Ordering::Relaxed);
        lock(&self.running).push((number, job.clone()));
        let step = match plan.disclosure().locality {
            Locality::Remote => REMOTE_STEP,
            Locality::Local => STEP,
        };
        let done = job.wait(step, limit);
        lock(&self.running).retain(|(n, _)| *n != number);
        // `Job::wait` asks the job to stop at a timeout, but not after a poll that cannot pass, and
        // it does not say whether the stop worked. Ask again and report a job that will not stop.
        let done = done.map_err(|e| {
            if stop(&job) {
                e.message
            } else {
                format!("{} {NOT_STOPPED}", e.message)
            }
        })?;
        match done.state {
            JobState::Succeeded => Ok(outputs(done.events)),
            JobState::Failed { error } => Err(error.message),
            JobState::Cancelled => Err("Cancelled.".to_owned()),
            _ => Err("The job did not finish.".to_owned()),
        }
    }

    /// The handle that cancels every job [`run`](Self::run) is waiting for, from any thread.
    pub fn canceller(&self) -> Canceller {
        Canceller(self.running.clone())
    }

    /// The client behind the view, for `plan`, `start` and the consent calls.
    pub fn client(&self) -> &AiClient {
        &self.client
    }
}

/// What a finished job produced: its outputs, then any streamed text as one text output, so a text
/// tool's reply is not lost.
fn outputs(events: Vec<Event>) -> Vec<Output> {
    let mut text = String::new();
    let mut outputs = Vec::new();
    for event in events {
        match event {
            Event::Output(output) => outputs.push(output),
            Event::TextDelta { text: part } => text.push_str(&part),
            _ => {}
        }
    }
    if !text.is_empty() {
        outputs.push(Output {
            payload: Payload::text(text),
            label: None,
        });
    }
    outputs
}
