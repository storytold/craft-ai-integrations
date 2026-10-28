//! Jobs: a worker thread per request, and the shared cell the provider polls.
//!
//! Submit validates and builds the request on the caller's thread, so a bad request fails before
//! anything is sent, then hands it to a worker that writes into a [`JobCell`]. Polls read the cell.
//!
//! While a worker waits on the SDK, the transport under the SDK runs on that same thread, so the
//! worker names its cell in a thread-local [`with_sink`] and streamed text goes straight into it.
//!
//! Cancelling marks the job `Cancelled` at once and discards whatever the worker produces later.
//! A streamed reply is also closed at its next line, which ends the provider's work. A reply that
//! is not streamed cannot be interrupted mid-flight; its worker finishes and is discarded, within
//! the SDK's time budget.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use craft_ai_core::job::{Event, JobSnapshot, JobState};
use craft_ai_core::{AiError, ErrorKind, JobId, Usage};

/// Finished jobs kept for polling before the oldest are forgotten.
const KEEP_FINISHED: usize = 64;
/// Most events one job keeps while waiting to be polled: past it the job fails rather than grows.
pub(crate) const MAX_EVENTS: usize = 65_536;

struct Inner {
    state: JobState,
    events: Vec<Event>,
    usage: Option<Usage>,
}

/// One job's shared state. Terminal states are final: later writes are ignored.
pub(crate) struct JobCell {
    inner: Mutex<Inner>,
}

impl JobCell {
    fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                state: JobState::Queued,
                events: Vec::new(),
                usage: None,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn is_terminal(&self) -> bool {
        self.lock().state.is_terminal()
    }

    pub fn start(&self) {
        let mut g = self.lock();
        if g.state == JobState::Queued {
            g.state = JobState::Running { percent: None };
        }
    }

    pub fn push(&self, event: Event) {
        let mut g = self.lock();
        if g.state.is_terminal() {
            return;
        }
        if g.events.len() >= MAX_EVENTS {
            g.state = JobState::Failed {
                error: AiError::new(
                    ErrorKind::Provider,
                    "too many events are waiting; poll the job while it runs",
                ),
            };
            g.events.clear();
            return;
        }
        g.events.push(event);
    }

    pub fn succeed(&self, usage: Option<Usage>) {
        let mut g = self.lock();
        if !g.state.is_terminal() {
            g.state = JobState::Succeeded;
            g.usage = usage;
        }
    }

    pub fn fail(&self, error: AiError) {
        let mut g = self.lock();
        if !g.state.is_terminal() {
            g.state = JobState::Failed { error };
        }
    }

    pub fn cancel(&self) {
        let mut g = self.lock();
        if !g.state.is_terminal() {
            g.state = JobState::Cancelled;
            g.events.clear();
        }
    }

    /// The state now and the events since the last call.
    pub fn snapshot(&self) -> JobSnapshot {
        let mut g = self.lock();
        JobSnapshot {
            state: g.state.clone(),
            events: std::mem::take(&mut g.events),
            usage: g.usage.clone(),
        }
    }
}

/// All jobs of one provider.
pub(crate) struct Registry {
    next: AtomicU64,
    max_active: usize,
    jobs: Mutex<Vec<(JobId, Arc<JobCell>)>>,
}

impl Registry {
    pub fn new(max_active: usize) -> Self {
        Self {
            next: AtomicU64::new(1),
            max_active: max_active.max(1),
            jobs: Mutex::new(Vec::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Vec<(JobId, Arc<JobCell>)>> {
        self.jobs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn get(&self, id: &JobId) -> Option<Arc<JobCell>> {
        self.lock()
            .iter()
            .find(|(j, _)| j == id)
            .map(|(_, c)| Arc::clone(c))
    }

    /// Starts a worker that runs `work` and records how it ended.
    pub fn spawn(
        &self,
        work: impl FnOnce(&Arc<JobCell>) -> Result<Option<Usage>, AiError> + Send + 'static,
    ) -> Result<JobId, AiError> {
        let cell = Arc::new(JobCell::new());
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let id = JobId::new(format!("fantastico-{n}"))
            .map_err(|e| AiError::new(ErrorKind::Internal, e.to_string()))?;
        {
            let mut jobs = self.lock();
            if jobs.iter().filter(|(_, c)| !c.is_terminal()).count() >= self.max_active {
                return Err(AiError::new(
                    ErrorKind::RateLimited,
                    "too many jobs are running on this FANTASTICO connection; wait for one to finish",
                ));
            }
            jobs.push((id.clone(), Arc::clone(&cell)));
            while jobs.len() > KEEP_FINISHED {
                match jobs.iter().position(|(_, c)| c.is_terminal()) {
                    Some(i) => {
                        jobs.remove(i);
                    }
                    None => break,
                }
            }
        }
        let worker_cell = Arc::clone(&cell);
        let spawned = std::thread::Builder::new()
            .name(format!("craft-ai-{id}"))
            .spawn(move || {
                worker_cell.start();
                match work(&worker_cell) {
                    Ok(usage) => worker_cell.succeed(usage),
                    Err(e) => worker_cell.fail(e),
                }
            });
        if spawned.is_err() {
            self.lock().retain(|(j, _)| j != &id);
            return Err(AiError::new(
                ErrorKind::Internal,
                "could not start a worker thread",
            ));
        }
        Ok(id)
    }
}

/// Where the streaming transport on this thread delivers text.
struct Sink {
    cell: Arc<JobCell>,
    /// False for a structured request: its text is JSON, delivered whole at the end.
    deltas: bool,
}

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

fn set_sink(sink: Option<Sink>) {
    SINK.with(|s| {
        if let Ok(mut slot) = s.try_borrow_mut() {
            *slot = sink;
        }
    });
}

/// Runs `f` with this thread's streamed text going to `cell`.
pub(crate) fn with_sink<R>(cell: &Arc<JobCell>, deltas: bool, f: impl FnOnce() -> R) -> R {
    set_sink(Some(Sink {
        cell: Arc::clone(cell),
        deltas,
    }));
    let r = f();
    set_sink(None);
    r
}

/// A piece of streamed text, for the job this thread works for (if any).
pub(crate) fn emit_text(text: &str) {
    SINK.with(|s| {
        if let Ok(slot) = s.try_borrow()
            && let Some(sink) = slot.as_ref().filter(|k| k.deltas)
        {
            sink.cell.push(Event::TextDelta {
                text: text.to_owned(),
            });
        }
    });
}

/// Whether the job this thread works for has ended (cancelled, or failed on its backlog).
pub(crate) fn sink_stopped() -> bool {
    SINK.with(|s| {
        s.try_borrow()
            .ok()
            .and_then(|slot| slot.as_ref().map(|k| k.cell.is_terminal()))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Event {
        Event::TextDelta { text: s.into() }
    }

    #[test]
    fn events_are_delivered_once_and_terminal_states_are_final() {
        let c = JobCell::new();
        c.start();
        c.push(text("a"));
        assert_eq!(c.snapshot().events.len(), 1);
        assert!(c.snapshot().events.is_empty());
        c.cancel();
        c.push(text("late"));
        c.succeed(None);
        c.fail(AiError::new(ErrorKind::Provider, "late"));
        let s = c.snapshot();
        assert_eq!(s.state, JobState::Cancelled);
        assert!(s.events.is_empty());
    }

    #[test]
    fn a_backlog_past_the_cap_fails_the_job() {
        let c = JobCell::new();
        c.start();
        for _ in 0..MAX_EVENTS {
            c.push(text("a"));
        }
        c.push(text("one too many"));
        let s = c.snapshot();
        assert!(matches!(s.state, JobState::Failed { .. }));
        assert!(s.events.is_empty());
    }

    #[test]
    fn the_sink_reaches_only_its_own_thread_and_only_while_set() {
        let cell = Arc::new(JobCell::new());
        cell.start();
        emit_text("before");
        with_sink(&cell, true, || {
            emit_text("in");
            std::thread::scope(|s| {
                s.spawn(|| emit_text("other thread"));
            });
            assert!(!sink_stopped());
        });
        emit_text("after");
        let events = cell.snapshot().events;
        assert_eq!(events, [text("in")]);
        with_sink(&cell, false, || emit_text("structured"));
        assert!(cell.snapshot().events.is_empty());
        cell.cancel();
        with_sink(&cell, true, || assert!(sink_stopped()));
        assert!(!sink_stopped());
    }
}
