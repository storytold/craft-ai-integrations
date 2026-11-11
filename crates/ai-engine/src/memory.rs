//! Models kept in memory: what each one holds, letting go of them, and refusing a load that will
//! not fit.
//!
//! A model stays loaded after its first run, because loading is the slow part. Nothing here runs
//! on a timer: the app asks what is loaded, asks to unload, or asks to unload what has sat idle,
//! whenever it chooses.
//!
//! Invariants: an unload never interrupts a run. A model a job holds is marked, and the job lets
//! go of it as it finishes. A [`Residency`] says "loaded" exactly when its slot holds a model.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};
use std::time::{Duration, Instant};

use craft_ai_core::{AiError, ModelId};
use craft_ai_features::gigabytes;

use crate::runner::LoadedModel;

/// Reads how much memory this computer has free, in bytes. `None` when it cannot tell, and then
/// no load is refused.
pub type FreeMemory = Arc<dyn Fn() -> Option<u64> + Send + Sync>;

/// One model held in memory, for a list a person can read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedModelInfo {
    pub model: ModelId,
    /// The model's name as a person sees it.
    pub label: String,
    /// Roughly how much memory it holds.
    pub bytes: u64,
    /// True when `bytes` is a stand-in (the weights file's size), because the model did not say.
    pub estimated: bool,
    /// A job is using it now, so an unload waits for that job to finish.
    pub running: bool,
    /// How long since a job last finished with it.
    pub idle: Duration,
}

/// What an unload did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unloaded {
    /// Let go of now.
    pub freed: Vec<ModelId>,
    /// In use by a job; let go of as that job finishes.
    pub after_run: Vec<ModelId>,
}

/// The slot a loaded model lives in. A job holds it for its whole turn.
pub(crate) type Slot = Mutex<Option<Box<dyn LoadedModel>>>;

#[derive(Default)]
struct State {
    /// Bytes held, and whether that figure is an estimate. `Some` exactly while loaded.
    held: Option<(u64, bool)>,
    /// A job holds the slot.
    running: bool,
    last_used: Option<Instant>,
    unload_requested: bool,
}

/// What [`Residency::unload`] found.
pub(crate) enum Outcome {
    /// Taken out of its slot; dropping it frees the memory.
    Freed(Box<dyn LoadedModel>),
    /// A job holds it; it is let go of as that job finishes.
    AfterRun,
    /// Not loaded, or not idle long enough.
    Untouched,
}

/// The memory side of one entry, readable without waiting for a run to finish.
///
/// Lock order: a job holds the slot, then takes this lock. [`Residency::unload`] holds this lock
/// and only ever *tries* the slot, so the two never wait on each other. A slot it cannot take
/// means a job holds it, which is how a running model is told apart. A job reads the unload
/// request under this lock as it leaves, so an unload either finds the slot free or leaves a
/// request the holder is certain to see.
#[derive(Default)]
pub(crate) struct Residency(Mutex<State>);

impl Residency {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Called by a job as soon as it holds the slot.
    pub(crate) fn enter(&self) {
        self.lock().running = true;
    }

    pub(crate) fn loaded(&self, bytes: u64, estimated: bool) {
        self.lock().held = Some((bytes, estimated));
    }

    /// Called by a job before it lets go of the slot, however its turn ended. Empties the slot
    /// when `discard` is set (the model was left in a bad state) or an unload was asked for
    /// meanwhile, and returns the model taken out so the caller drops it after unlocking.
    pub(crate) fn leave(
        &self,
        slot: &mut Option<Box<dyn LoadedModel>>,
        discard: bool,
    ) -> Option<Box<dyn LoadedModel>> {
        let mut state = self.lock();
        state.running = false;
        state.last_used = Some(Instant::now());
        if !(discard || state.unload_requested) {
            return None;
        }
        state.unload_requested = false;
        state.held = None;
        slot.take()
    }

    /// Unloads the model in `slot`. With `idle`, only one unused for at least that long; a busy
    /// one is then left alone rather than marked.
    pub(crate) fn unload(&self, slot: &Slot, idle: Option<Duration>) -> Outcome {
        let mut state = self.lock();
        if let Some(older_than) = idle {
            let rested = state.last_used.is_none_or(|at| at.elapsed() >= older_than);
            if state.held.is_none() || !rested {
                return Outcome::Untouched;
            }
        }
        let mut held = match slot.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            // A job holds the slot: it is running, or has just taken the slot. It reads this
            // request when it leaves. The model may not be loaded yet; then it never stays.
            Err(TryLockError::WouldBlock) => {
                if idle.is_some() {
                    return Outcome::Untouched;
                }
                state.unload_requested = true;
                return Outcome::AfterRun;
            }
        };
        state.held = None;
        state.unload_requested = false;
        held.take().map_or(Outcome::Untouched, Outcome::Freed)
    }

    pub(crate) fn info(&self, model: &ModelId, label: &str) -> Option<LoadedModelInfo> {
        let state = self.lock();
        let (bytes, estimated) = state.held?;
        Some(LoadedModelInfo {
            model: model.clone(),
            label: label.to_owned(),
            bytes,
            estimated,
            running: state.running,
            idle: state.last_used.map(|at| at.elapsed()).unwrap_or_default(),
        })
    }
}

/// The weights file's size, the stand-in for a model's memory when its runner does not say.
pub(crate) fn file_size(weights: &Path) -> Option<u64> {
    std::fs::metadata(weights).ok().map(|m| m.len())
}

/// Refuses a load of `needed` bytes when `free` reports less. Says nothing when either is
/// unknown, since a wrong refusal is worse than a slow load.
pub(crate) fn check_fits(
    label: &str,
    needed: Option<u64>,
    free: Option<&FreeMemory>,
) -> Result<(), AiError> {
    let (Some(needed), Some(free)) = (needed, free.and_then(|read| read())) else {
        return Ok(());
    };
    if needed <= free {
        return Ok(());
    }
    Err(AiError::unsupported(format!(
        "There isn't enough free memory to load {label}. It needs about {}, and about {} is free. \
         Unload other AI models or close other programs, then try again.",
        gigabytes(needed),
        gigabytes(free),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn free(bytes: Option<u64>) -> FreeMemory {
        Arc::new(move || bytes)
    }

    #[test]
    fn a_load_that_fits_or_cannot_be_judged_goes_ahead() {
        assert!(check_fits("M", Some(10), Some(&free(Some(10)))).is_ok());
        assert!(check_fits("M", Some(10), Some(&free(Some(20)))).is_ok());
        assert!(check_fits("M", None, Some(&free(Some(1)))).is_ok());
        assert!(check_fits("M", Some(10), Some(&free(None))).is_ok());
        assert!(check_fits("M", Some(10), None).is_ok());
    }

    #[test]
    fn a_load_that_will_not_fit_says_both_sizes_in_words() {
        let err = check_fits(
            "Remove background",
            Some(8_000_000_000),
            Some(&free(Some(2_500_000_000))),
        )
        .unwrap_err();
        assert_eq!(err.kind, craft_ai_core::ErrorKind::Unsupported);
        assert!(err.message.contains("Remove background"), "{}", err.message);
        assert!(err.message.contains("8.0 GB"), "{}", err.message);
        assert!(err.message.contains("2.5 GB"), "{}", err.message);
    }

    struct Held;

    impl LoadedModel for Held {
        fn run(
            &mut self,
            _: &craft_ai_core::Request,
            _: &crate::RunContext,
        ) -> Result<Vec<craft_ai_core::Output>, AiError> {
            Ok(Vec::new())
        }
    }

    fn loaded() -> (Residency, Slot) {
        let r = Residency::default();
        r.loaded(5, true);
        (r, Mutex::new(Some(Box::new(Held))))
    }

    fn model() -> ModelId {
        ModelId::new("m").unwrap()
    }

    /// An unload during a run only marks the model; the run's own exit then empties the slot.
    /// Without the request check in `leave`, the model would stay loaded after the run.
    #[test]
    fn an_unload_during_a_run_waits_for_the_run_to_leave() {
        let (r, slot) = loaded();
        let mut held = slot.lock().unwrap();
        r.enter();
        assert!(matches!(r.unload(&slot, None), Outcome::AfterRun));
        assert!(r.info(&model(), "M").is_some(), "still loaded mid-run");
        assert!(r.leave(&mut held, false).is_some(), "the run let go of it");
        assert!(held.is_none());
        assert!(r.info(&model(), "M").is_none());
    }

    /// A job that has taken the slot but not yet marked itself running still gets the request.
    #[test]
    fn an_unload_between_taking_the_slot_and_entering_is_not_lost() {
        let (r, slot) = loaded();
        let mut held = slot.lock().unwrap();
        assert!(matches!(r.unload(&slot, None), Outcome::AfterRun));
        r.enter();
        assert!(r.leave(&mut held, false).is_some());
    }

    #[test]
    fn idle_unload_skips_busy_and_recent_models_and_frees_rested_ones() {
        let (r, slot) = loaded();
        {
            let mut held = slot.lock().unwrap();
            r.enter();
            assert!(matches!(
                r.unload(&slot, Some(Duration::ZERO)),
                Outcome::Untouched
            ));
            assert!(
                r.leave(&mut held, false).is_none(),
                "an idle sweep leaves no request"
            );
        }
        assert!(matches!(
            r.unload(&slot, Some(Duration::from_secs(3600))),
            Outcome::Untouched
        ));
        assert!(matches!(
            r.unload(&slot, Some(Duration::ZERO)),
            Outcome::Freed(_)
        ));
        assert!(slot.lock().unwrap().is_none());
        assert!(r.info(&model(), "M").is_none());
        assert!(
            matches!(r.unload(&slot, None), Outcome::Untouched),
            "nothing left to free"
        );
    }

    /// A model a failed run left in a bad state is let go of even with no unload asked.
    #[test]
    fn a_discarded_model_is_emptied_on_leave() {
        let (r, slot) = loaded();
        let mut held = slot.lock().unwrap();
        r.enter();
        assert!(r.leave(&mut held, true).is_some());
        assert!(r.info(&model(), "M").is_none());
    }
}
