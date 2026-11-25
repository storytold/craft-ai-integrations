//! What a model author writes: a [`ModelRunner`] that loads weights, and the [`LoadedModel`] it
//! produces. Nothing else about running a model on this computer is the author's job. Job
//! numbers, threads, polling, cancelling, progress, consent and keys are handled around it.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use craft_ai_core::{AiError, Event, ModelInfo, Output, Request};
use craft_ai_features::GpuVendor;

/// One model on one engine, for example SAM 3 on Candle.
///
/// A runner is a description plus a loader. It holds no weights itself, so registering one costs
/// nothing until a job needs it.
pub trait ModelRunner: Send + Sync {
    /// The engine this runner belongs to, as written in the catalogue's `NativeRoute.provider`
    /// (`candle`, `burn`). Runners with the same engine name are served by one provider.
    fn engine(&self) -> &str;

    /// The model as a tool sees it: its id (the catalogue's `NativeRoute.model`) and what it
    /// accepts. The provider fills in the weights path, so leave `local_file` empty.
    fn model(&self) -> ModelInfo;

    /// The card makers this build of the engine can use. Empty means the processor only. Engine
    /// advice reads this, so an app never has to state it.
    fn accelerators(&self) -> &[GpuVendor] {
        &[]
    }

    /// Reads the weights. Called once per weights file and kept, because loading is the slow
    /// part. Return `Unsupported` for a file that is not this model, with words a person can act
    /// on, since the message reaches the screen.
    fn load(&self, weights: &Path) -> Result<Box<dyn LoadedModel>, AiError>;

    /// Roughly how much memory loading `weights` will take, checked against free memory before a
    /// load. The default is the file's size; override it when the model needs much more (a
    /// working buffer) or the file is compressed. `None` skips the check.
    fn memory_needed(&self, weights: &Path) -> Option<u64> {
        crate::memory::file_size(weights)
    }
}

/// A model that is in memory and ready to run.
///
/// Jobs for one model run one at a time (a graphics card rarely likes two), so `run` may use
/// `&mut self` for scratch buffers and needs no locking of its own.
pub trait LoadedModel: Send {
    /// Runs one already-validated request. The request matches the spec from
    /// [`ModelRunner::model`], so inputs and counts need no re-checking, only decoding.
    ///
    /// Call [`RunContext::progress`] as work advances and [`RunContext::check_cancelled`] between
    /// steps, so Cancel is quick.
    fn run(&mut self, request: &Request, ctx: &RunContext) -> Result<Vec<Output>, AiError>;

    /// Roughly how much memory this model holds now, for a person deciding what to unload.
    /// `None` (the default) shows the weights file's size instead, marked as an estimate.
    fn memory_bytes(&self) -> Option<u64> {
        None
    }
}

/// What a running job lets the runner do: report, listen for Cancel, say something.
#[derive(Clone)]
pub struct RunContext {
    cancelled: Arc<AtomicBool>,
    percent: Arc<AtomicU8>,
    notices: Arc<Mutex<Vec<Event>>>,
}

impl RunContext {
    pub(crate) fn new(
        cancelled: Arc<AtomicBool>,
        percent: Arc<AtomicU8>,
        notices: Arc<Mutex<Vec<Event>>>,
    ) -> Self {
        Self {
            cancelled,
            percent,
            notices,
        }
    }

    /// A context for running a [`LoadedModel`] directly in a test.
    pub fn detached() -> Self {
        Self::new(
            Arc::default(),
            Arc::default(),
            Arc::new(Mutex::new(Vec::new())),
        )
    }

    /// Whether the person pressed Cancel.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// `Err(cancelled)` once Cancel is pressed. Use as `ctx.check_cancelled()?`.
    pub fn check_cancelled(&self) -> Result<(), AiError> {
        if self.is_cancelled() {
            Err(AiError::cancelled())
        } else {
            Ok(())
        }
    }

    /// Reports how far along the job is, 0 to 100. It never goes backwards and never reaches 100
    /// (the job finishing says that), so callers may report in any order.
    pub fn progress(&self, percent: u8) {
        self.percent.fetch_max(percent.min(99), Ordering::AcqRel);
    }

    /// Tells the person something that is not an error, such as "using the processor because no
    /// graphics card was found". Shown once, in plain words.
    pub fn notice(&self, message: impl Into<String>) {
        let mut events = self.notices.lock().unwrap_or_else(PoisonError::into_inner);
        events.push(Event::Notice {
            message: message.into(),
        });
    }
}
