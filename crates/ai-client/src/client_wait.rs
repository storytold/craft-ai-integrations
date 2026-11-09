//! Waiting for a job to end, and how hard sending and polling try again.
//!
//! Invariants:
//! - Sending a job is tried again only after a rate limit. The provider refused that request, so
//!   nothing was accepted. A dropped connection or a server error while sending is never
//!   retried here: the job may already be running, and a second send could charge twice.
//! - A poll that fails with a retryable error (a dropped connection, a rate limit) does not end
//!   the wait. Polling only reads, so another try is safe. A run of such failures ends the wait
//!   once [`POLL_RETRY`]'s tries or budget run out; a success resets the run. The budget counts
//!   from the start of the first failed poll, and covers the whole pause, `interval` included.
//! - The overall `timeout` still wins: no pause, retry or not, sleeps past it, and no poll starts
//!   after it, the first included. A wait that ends past it, however it ends, cancels the job and
//!   records the give-up.
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
    /// [`Backoff::NEVER`] to turn either off. The send rule also goes to the worker-backed
    /// adapters the current factory builds (see
    /// [`ProviderFactory::set_send_retry`](crate::ProviderFactory::set_send_retry)), so call
    /// this after [`with_factory`](Self::with_factory).
    pub fn with_retries(mut self, submit: Backoff, poll: Backoff) -> Self {
        self.submit_retry = submit;
        self.poll_retry = poll;
        self.factory.set_send_retry(submit);
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
            if Instant::now() >= deadline {
                return Err(self.give_up());
            }
            let asked = Instant::now();
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
                        streak = asked;
                    }
                    failures = failures.saturating_add(1);
                    let elapsed = streak.elapsed();
                    let pause = self
                        .poll_retry
                        .next_wait(failures, &error, elapsed)
                        .map(|wait| wait.max(interval))
                        .filter(|pause| elapsed.saturating_add(*pause) <= self.poll_retry.budget);
                    match pause {
                        Some(pause) => pause,
                        None if Instant::now() >= deadline => return Err(self.give_up()),
                        None => return Err(tried(error, failures)),
                    }
                }
                Err(error) => return Err(error),
            };
            let now = Instant::now();
            std::thread::sleep(pause.min(deadline.saturating_duration_since(now)));
        }
    }

    /// Ends a wait that ran out of time: asks the provider to stop and records the give-up.
    fn give_up(&self) -> AiError {
        let _ = self.cancel();
        if let Some(recorder) = &self.recorder {
            recorder.gave_up();
        }
        AiError::new(ErrorKind::Network, "the job took too long")
    }
}
