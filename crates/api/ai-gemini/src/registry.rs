//! The Gemini adapter's job registry and the rules that keep it bounded.
//!
//! A fill's picture waits here until a poll collects it. Three limits hold the memory down:
//! - count: at most [`MAX_TRACKED`] jobs. Finished jobs already handed out are forgotten first.
//! - age: a finished job nobody collected is forgotten [`UNCOLLECTED_TTL`] after it ended.
//!   A caller that comes back later gets "unknown Gemini job".
//! - bytes: no new fill starts while uncollected pictures hold [`MAX_HELD_BYTES`] or more.
//!
//! Before these, only the count was bounded: 256 uncollected 4K answers could hold about 12 GB.

use craft_ai_core::{AiError, ErrorKind, Event, JobId, JobState};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

/// Jobs remembered in all, running or finished.
pub(crate) const MAX_TRACKED: usize = 256;
/// How long a finished, uncollected job is kept.
pub(crate) const UNCOLLECTED_TTL: Duration = Duration::from_secs(60 * 60);
/// Uncollected picture bytes at which new fills are refused.
pub(crate) const MAX_HELD_BYTES: u64 = 512 * 1024 * 1024;

pub(crate) struct Slot {
    pub(crate) state: JobState,
    /// Events not yet handed out by a poll.
    pub(crate) events: Vec<Event>,
    /// Whether a poll has delivered the terminal state, including failure or cancellation.
    pub(crate) terminal_delivered: bool,
    /// When the job reached its terminal state; `None` while it runs.
    pub(crate) ended: Option<Instant>,
}

impl Slot {
    /// A job that has just been submitted.
    pub(crate) fn running() -> Self {
        Self {
            state: JobState::Running { percent: None },
            events: Vec::new(),
            terminal_delivered: false,
            ended: None,
        }
    }

    /// Moves the job to a terminal state and records when, so the age limit can apply.
    pub(crate) fn finish(&mut self, state: JobState, now: Instant) {
        self.state = state;
        self.ended = Some(now);
    }

    /// Bytes of output this slot holds that no poll has collected yet.
    fn held_bytes(&self) -> u64 {
        self.events
            .iter()
            .map(|e| match e {
                Event::Output(o) => o.payload.size_hint().unwrap_or(0),
                _ => 0,
            })
            .sum()
    }
}

pub(crate) type Jobs = Arc<Mutex<HashMap<JobId, Slot>>>;

/// Locks the registry, or reports it unavailable if a thread panicked while holding it.
pub(crate) fn lock(jobs: &Jobs) -> Result<MutexGuard<'_, HashMap<JobId, Slot>>, AiError> {
    jobs.lock()
        .map_err(|_| AiError::new(ErrorKind::Internal, "Gemini job registry unavailable"))
}

/// Makes room for one more job, or says why there is none.
///
/// Expired uncollected jobs are always forgotten. Delivered finished jobs are forgotten only when
/// the count is full, so a caller can still poll a result it has already seen. Running jobs are
/// never forgotten. A full registry is `Quota`, not `RateLimited`: only the caller collecting a
/// result frees room, so trying again after a pause cannot help.
pub(crate) fn admit(jobs: &mut HashMap<JobId, Slot>, now: Instant) -> Result<(), AiError> {
    jobs.retain(|_, s| {
        !s.ended
            .is_some_and(|t| now.saturating_duration_since(t) >= UNCOLLECTED_TTL)
    });
    if jobs.len() >= MAX_TRACKED {
        jobs.retain(|_, s| !s.terminal_delivered);
    }
    if jobs.len() >= MAX_TRACKED {
        return Err(AiError::new(
            ErrorKind::Quota,
            "too many Gemini results are waiting; check a finished fill first",
        ));
    }
    if jobs.values().map(Slot::held_bytes).sum::<u64>() >= MAX_HELD_BYTES {
        return Err(AiError::new(
            ErrorKind::Quota,
            "finished Gemini pictures are waiting to be collected; check a finished fill first",
        ));
    }
    Ok(())
}
