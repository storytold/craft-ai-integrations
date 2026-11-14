//! One job in flight: its state, its progress, and the Cancel flag the runner listens to.
//!
//! Invariants: a finished job never changes again (a Cancel that arrives after the runner has
//! finished is ignored, and a result that arrives after Cancel is thrown away); progress only
//! goes up; every event is handed out by exactly one poll.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use craft_ai_core::{AiError, ErrorKind, Event, JobSnapshot, JobState, Output};

use crate::runner::RunContext;

enum Phase {
    Queued,
    Running,
    Done(JobState),
}

struct Inner {
    phase: Phase,
    events: Vec<Event>,
}

pub(crate) struct Job {
    cancelled: Arc<AtomicBool>,
    percent: Arc<AtomicU8>,
    notices: Arc<Mutex<Vec<Event>>>,
    inner: Mutex<Inner>,
}

impl Job {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            cancelled: Arc::default(),
            percent: Arc::default(),
            notices: Arc::new(Mutex::new(Vec::new())),
            inner: Mutex::new(Inner {
                phase: Phase::Queued,
                events: Vec::new(),
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn context(&self) -> RunContext {
        RunContext::new(
            self.cancelled.clone(),
            self.percent.clone(),
            self.notices.clone(),
        )
    }

    pub(crate) fn is_done(&self) -> bool {
        matches!(self.lock().phase, Phase::Done(_))
    }

    /// Marks the job as running. False when it was cancelled before it got the chance.
    pub(crate) fn begin(&self) -> bool {
        let mut inner = self.lock();
        match inner.phase {
            Phase::Queued => {
                inner.phase = Phase::Running;
                true
            }
            Phase::Running => true,
            Phase::Done(_) => false,
        }
    }

    pub(crate) fn finish(&self, result: Result<Vec<Output>, AiError>) {
        let mut inner = self.lock();
        if matches!(inner.phase, Phase::Done(_)) {
            return;
        }
        inner.phase = Phase::Done(match result {
            Ok(outputs) => {
                inner.events.extend(outputs.into_iter().map(Event::Output));
                JobState::Succeeded
            }
            Err(e) if e.kind == ErrorKind::Cancelled => JobState::Cancelled,
            Err(error) => JobState::Failed { error },
        });
    }

    /// Asks the runner to stop and reports the job as cancelled straight away. Idempotent.
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        let mut inner = self.lock();
        if !matches!(inner.phase, Phase::Done(_)) {
            inner.phase = Phase::Done(JobState::Cancelled);
        }
    }

    pub(crate) fn snapshot(&self) -> JobSnapshot {
        let mut inner = self.lock();
        let mut events: Vec<Event> =
            std::mem::take(&mut *self.notices.lock().unwrap_or_else(PoisonError::into_inner));
        events.append(&mut inner.events);
        let state = match &inner.phase {
            Phase::Queued => JobState::Queued,
            Phase::Running => JobState::Running {
                percent: Some(self.percent.load(Ordering::Acquire)),
            },
            Phase::Done(state) => state.clone(),
        };
        JobSnapshot {
            state,
            events,
            usage: None,
        }
    }
}
