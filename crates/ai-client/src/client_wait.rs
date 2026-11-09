//! Waiting for a job to end, and how hard sending and polling try again.
//!
//! Invariants:
//! - Sending a job is tried again only after a rate limit. The provider refused that request, so
//!   nothing was accepted. A dropped connection or a server error while sending is never
//!   retried here: the job may already be running, and a second send could charge twice.
//! - A poll that fails with a retryable error (a dropped connection, a rate limit) does not end
//!   the wait. Polling only reads, so another try is safe. A run of such failures ends the wait
//!   once [`POLL_RETRY`]'s tries or budget run out; a success resets the run.
//! - The overall `timeout` still wins: no pause, retry or not, sleeps past it.
use std::time::{Duration, Instant};

use craft_ai_core::retry::tried;
use craft_ai_core::{AiError, Backoff, ErrorKind};

use super::{AiClient, Finished, Job};

/// Polls after a failed poll: five tries, one second doubling, a minute in total.
pub(crate) const POLL_RETRY: Backoff = Backoff {
    max_attempts: 5,
    first_delay: Duration::from_secs(1),
    budget: Duration::from_secs(60),
};

impl AiClient {
    /// Sets how sending a job retries after a rate limit (default [`Backoff::STANDARD`]) and how
    /// [`Job::wait`] retries a failed poll (default five tries within a minute). Pass
    /// [`Backoff::NEVER`] to turn either off.
    pub fn with_retries(mut self, submit: Backoff, poll: Backoff) -> Self {
        self.submit_retry = submit;
        self.poll_retry = poll;
        self
    }

    /// The submit retry rule [`start`](Self::start) uses.
    pub(crate) fn submit_retry(&self) -> Backoff {
        self.submit_retry
    }

    /// The poll retry rule a started [`Job`] carries.
    pub(crate) fn poll_retry(&self) -> Backoff {
        self.poll_retry
    }
}

/// True for the one sending failure that is safe to try again: the provider refused it.
pub(crate) fn submit_worth_retrying(error: &AiError) -> bool {
    error.kind == ErrorKind::RateLimited
}

impl Job {
    /// Polls every `interval` until the job ends or `timeout` passes, collecting events. A
    /// retryable poll failure pauses and tries again (see the module notes). For scripts and
    /// tests; an app with its own job system polls instead.
    pub fn wait(&self, interval: Duration, timeout: Duration) -> Result<Finished, AiError> {
        let deadline = Instant::now() + timeout;
        let mut events = Vec::new();
        let mut failures = 0u8;
        let mut streak = Instant::now();
        loop {
            let pause = match self.poll() {
                Ok(snap) => {
                    failures = 0;
                    events.extend(snap.events);
                    if snap.state.is_terminal() {
                        return Ok(Finished {
                            state: snap.state,
                            events,
                            usage: snap.usage,
                        });
                    }
                    interval
                }
                Err(error) if error.is_retryable() => {
                    if failures == 0 {
                        streak = Instant::now();
                    }
                    failures = failures.saturating_add(1);
                    match self
                        .poll_retry
                        .next_wait(failures, &error, streak.elapsed())
                    {
                        Some(wait) => wait.max(interval),
                        None => return Err(tried(error, failures)),
                    }
                }
                Err(error) => return Err(error),
            };
            let now = Instant::now();
            if now >= deadline {
                let _ = self.cancel();
                if let Some(recorder) = &self.recorder {
                    recorder.gave_up();
                }
                return Err(AiError::new(ErrorKind::Network, "the job took too long"));
            }
            std::thread::sleep(pause.min(deadline - now));
        }
    }
}
