//! The provider every built-in engine shares. It turns a [`ModelRunner`] into a [`Provider`]:
//! job numbers, a worker thread per job, polling, Cancel, progress, validation against the
//! model's spec, loading weights once, and a runner that panics becoming a failed job instead of
//! a crashed app.

use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use craft_ai_core::{
    AiError, Connection, ErrorKind, JobId, JobSnapshot, Limits, ModelId, ModelInfo, Output,
    Provider, Request,
};

use crate::job::Job;
use crate::registry::EngineRegistry;
use crate::runner::{LoadedModel, ModelRunner};

/// Finished jobs kept so a late poll still gets its answer.
const KEEP_FINISHED: usize = 64;

/// Most jobs that may be waiting or running at once. Each holds its own copy of the request
/// (a picture, often) and a thread, so a burst is refused with a plain sentence instead.
const MAX_PENDING: usize = 16;

/// Counts a job as pending until it is dropped, however the job ends.
struct Pending(Arc<AtomicUsize>);

impl Drop for Pending {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Entry {
    runner: Arc<dyn ModelRunner>,
    weights: PathBuf,
    model: ModelInfo,
    /// The loaded model. Held for the length of a run, which is what makes jobs for one model
    /// take turns.
    loaded: Mutex<Option<Box<dyn LoadedModel>>>,
}

impl Entry {
    fn run(&self, request: &Request, job: &Job) -> Result<Vec<Output>, AiError> {
        let mut slot = self.loaded.lock().unwrap_or_else(PoisonError::into_inner);
        // Only now is the job really running: until this point it was waiting its turn behind
        // another job for the same model. A job cancelled while it waited never loads or runs.
        if !job.begin() {
            return Err(AiError::cancelled());
        }
        let ctx = job.context();
        if slot.is_none() {
            let weights = self.weights.clone();
            *slot = Some(guarded(|| self.runner.load(&weights))?);
        }
        // Cancel pressed during a slow load stops here, before any work on the request.
        ctx.check_cancelled()?;
        let Some(model) = slot.as_mut() else {
            return Err(AiError::new(ErrorKind::Internal, "the model did not load"));
        };
        let result = guarded(|| model.run(request, &ctx));
        if matches!(&result, Err(e) if e.kind == ErrorKind::Internal) {
            // Whatever state it was left in, do not reuse it.
            *slot = None;
        }
        result
    }
}

/// Runs `f`, turning a panic into an error a person can read.
fn guarded<T>(f: impl FnOnce() -> Result<T, AiError>) -> Result<T, AiError> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        Err(AiError::new(
            ErrorKind::Internal,
            "The built-in model stopped unexpectedly. Try again, or choose another in Set up AI.",
        ))
    })
}

#[derive(Default)]
struct Table {
    jobs: HashMap<JobId, Arc<Job>>,
    order: VecDeque<JobId>,
}

/// Runs registered models from weights files on this computer.
pub struct EngineProvider {
    engine: String,
    entries: Vec<Arc<Entry>>,
    limits: Limits,
    table: Mutex<Table>,
    next: AtomicU64,
    pending: Arc<AtomicUsize>,
}

impl EngineProvider {
    /// Serves `runners`, each reading its own weights file. Nothing is loaded until a job needs it.
    pub fn new(
        engine: impl Into<String>,
        runners: impl IntoIterator<Item = (Arc<dyn ModelRunner>, PathBuf)>,
    ) -> Self {
        let entries = runners
            .into_iter()
            .map(|(runner, weights)| {
                let mut model = runner.model();
                model.local_file = weights.to_str().map(str::to_owned);
                Arc::new(Entry {
                    runner,
                    weights,
                    model,
                    loaded: Mutex::new(None),
                })
            })
            .collect();
        Self {
            engine: engine.into(),
            entries,
            limits: Limits::default(),
            table: Mutex::new(Table::default()),
            next: AtomicU64::new(0),
            pending: Arc::default(),
        }
    }

    /// Builds the provider for a saved connection: one runner per model on it, each reading the
    /// weights file the launcher recorded. This is what a `ProviderFactory` calls.
    pub fn from_connection(
        registry: &EngineRegistry,
        connection: &Connection,
    ) -> Result<Self, AiError> {
        if connection.models.is_empty() {
            return Err(AiError::invalid(
                "This connection has no model to run. Open Set up AI to fix it.",
            ));
        }
        let mut runners = Vec::new();
        for model in &connection.models {
            let runner = registry
                .find(&connection.provider, model.id.as_str())
                .ok_or_else(|| {
                    AiError::unsupported(format!(
                        "This app can't run {} with the {} engine yet.",
                        model.label, connection.provider
                    ))
                })?;
            let file = model.local_file.as_deref().ok_or_else(|| {
                AiError::invalid(format!(
                    "{} isn't fully set up. Open Set up AI to fix it.",
                    model.label
                ))
            })?;
            runners.push((runner, PathBuf::from(file)));
        }
        Ok(Self::new(connection.provider.clone(), runners))
    }

    fn table(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn job(&self, id: &JobId) -> Result<Arc<Job>, AiError> {
        self.table()
            .jobs
            .get(id)
            .cloned()
            .ok_or_else(|| AiError::invalid(format!("unknown job {id}")))
    }

    fn remember(&self, id: JobId, job: Arc<Job>) {
        let mut table = self.table();
        table.jobs.insert(id.clone(), job);
        table.order.push_back(id);
        let mut drop_ids = table.jobs.len().saturating_sub(KEEP_FINISHED);
        let mut kept = VecDeque::new();
        while let Some(old) = table.order.pop_front() {
            let finished = table.jobs.get(&old).is_some_and(|j| j.is_done());
            if drop_ids > 0 && finished {
                table.jobs.remove(&old);
                drop_ids -= 1;
            } else {
                kept.push_back(old);
            }
        }
        table.order = kept;
    }
}

impl Provider for EngineProvider {
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        Ok(self.entries.iter().map(|e| e.model.clone()).collect())
    }

    fn submit(&self, model: &ModelId, request: &Request) -> Result<JobId, AiError> {
        let entry = self
            .entries
            .iter()
            .find(|e| &e.model.id == model)
            .ok_or_else(|| AiError::invalid(format!("unknown model {model}")))?
            .clone();
        let spec = entry.model.spec_for(&request.capability).ok_or_else(|| {
            AiError::unsupported(format!("model {model} cannot do {}", request.capability))
        })?;
        request.validate(spec, &self.limits)?;
        if !entry.weights.is_file() {
            return Err(AiError::unsupported(format!(
                "The file for {} is no longer there. Open Set up AI to fix it.",
                entry.model.label
            )));
        }
        if self.pending.fetch_add(1, Ordering::AcqRel) >= MAX_PENDING {
            self.pending.fetch_sub(1, Ordering::AcqRel);
            return Err(AiError::new(
                ErrorKind::RateLimited,
                "Too many requests are waiting for this model. Wait for some to finish, then try again.",
            ));
        }
        let pending = Pending(self.pending.clone());
        let n = self.next.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        let id = JobId::new(format!("{}-{n}", self.engine))
            .map_err(|e| AiError::new(ErrorKind::Internal, e.to_string()))?;
        let job = Job::new();
        let (work, request) = (job.clone(), request.clone());
        std::thread::Builder::new()
            .name(format!("{}-{n}", self.engine))
            .spawn(move || {
                let _pending = pending;
                let result = entry.run(&request, &work);
                work.finish(result);
            })
            .map_err(|e| {
                AiError::new(ErrorKind::Internal, format!("could not start the job: {e}"))
            })?;
        self.remember(id.clone(), job);
        Ok(id)
    }

    fn poll(&self, job: &JobId) -> Result<JobSnapshot, AiError> {
        Ok(self.job(job)?.snapshot())
    }

    fn cancel(&self, job: &JobId) -> Result<(), AiError> {
        self.job(job)?.cancel();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use craft_ai_core::JobState;

    use super::*;
    use crate::testing::FakeRunner;

    fn entry(fake: FakeRunner, dir: &tempfile::TempDir) -> Arc<Entry> {
        let weights = dir.path().join("m.safetensors");
        std::fs::write(&weights, b"w").unwrap();
        let provider = EngineProvider::new(
            "candle",
            [(Arc::new(fake) as Arc<dyn ModelRunner>, weights)],
        );
        provider.entries[0].clone()
    }

    /// The worker thread's first act is `Entry::run`. A job cancelled before that call must not
    /// load the weights or enter the runner, even when this entry is the first to touch them.
    /// The same entry then loads and runs an uncancelled job, so the two zeroes come from the
    /// cancel guard rather than from a fixture that could never have loaded or run anything.
    #[test]
    fn a_job_cancelled_before_its_turn_never_loads_or_runs_the_model() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = FakeRunner::new("candle", "sam3", "image.segment");
        fake.step_pause = Duration::ZERO;
        let (loads, runs) = (fake.loads.clone(), fake.runs.clone());
        let request = fake.good_request().unwrap();
        let entry = entry(fake, &dir);

        let cancelled = Job::new();
        cancelled.cancel();
        let result = entry.run(&request, &cancelled);
        assert_eq!(
            result.as_ref().err().map(|e| e.kind),
            Some(ErrorKind::Cancelled)
        );
        cancelled.finish(result);
        assert_eq!(cancelled.snapshot().state, JobState::Cancelled);
        assert_eq!(
            loads.load(Ordering::SeqCst),
            0,
            "a cancelled job must not load the model"
        );
        assert_eq!(
            runs.load(Ordering::SeqCst),
            0,
            "a cancelled job must not run the model"
        );

        // Positive control through the same production path. If the `job.begin()` guard were
        // removed, the cancelled job above would load (and, without the cancel check, run), so
        // these counters would no longer be 0.
        let control = Job::new();
        let result = entry.run(&request, &control);
        assert!(matches!(&result, Ok(outputs) if outputs.len() == 1));
        control.finish(result);
        assert_eq!(control.snapshot().state, JobState::Succeeded);
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }
}
