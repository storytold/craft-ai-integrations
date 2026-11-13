//! Waiting for a job to end, and how hard sending and polling try again.
//!
//! Invariants:
//! - Sending a job is tried again only after a rate limit. The provider refused that request, so
//!   nothing was accepted. A dropped connection or a server error while sending is never
//!   retried here: the job may already be running, and a second send could charge twice.
//! - A poll that fails with a retryable error (a dropped connection, a rate limit) does not end
//!   the wait. Polling only reads, so another try is safe. A run of such failures ends the wait
//!   once [`POLL_RETRY`]'s tries or budget run out; a success resets the run. The budget counts
//!   the retry pauses only: not the failed polls (a poll that times out can still be retried)
//!   and not `interval`, which is the floor of every pause (a long interval still retries).
//! - The overall `timeout` still wins: no pause, retry or not, sleeps past it, and no poll starts
//!   after it, the first included.
//! - A wait that gives up, at the timeout or because polls kept failing, cancels the job and
//!   records the give-up, so a paid job is not left running unwatched. A refusal (a poll error
//!   that cannot pass) is returned as it is.
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
    /// adapters the factory builds (see
    /// [`ProviderFactory::set_send_retry`](crate::ProviderFactory::set_send_retry)), including a
    /// factory set later with [`with_factory`](Self::with_factory).
    pub fn with_retries(mut self, submit: Backoff, poll: Backoff) -> Self {
        self.submit_retry = submit;
        self.poll_retry = poll;
        self.send_rule_chosen = true;
        self.factory.set_send_retry(submit);
        self
    }

    /// True once [`with_retries`](Self::with_retries) chose the send rule, so a factory set
    /// afterwards takes it too. Until then a factory keeps the rule it was built with.
    pub(crate) fn send_rule_chosen(&self) -> bool {
        self.send_rule_chosen
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

/// The longest wait a deadline is set for. `Duration::MAX` is the usual way to say "no limit",
/// and adding it to an `Instant` overflows, so a longer `timeout` waits this long instead.
const LONGEST_WAIT: Duration = Duration::from_secs(10 * 365 * 24 * 60 * 60);

/// `now + timeout`, with a timeout too long to add treated as [`LONGEST_WAIT`]. If even that
/// cannot be added the deadline is `now`, so the wait gives up and cancels the job rather than
/// panicking with the job already started.
pub(crate) fn deadline_after(now: Instant, timeout: Duration) -> Instant {
    now.checked_add(timeout.min(LONGEST_WAIT)).unwrap_or(now)
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
        let deadline = deadline_after(Instant::now(), timeout);
        let mut events = Vec::new();
        let mut failures = 0u8;
        let mut paused = Duration::ZERO;
        loop {
            if Instant::now() >= deadline {
                return Err(self.give_up(took_too_long()));
            }
            let pause = match self.poll() {
                Ok(snap) => {
                    (failures, paused) = (0, Duration::ZERO);
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
                    failures = failures.saturating_add(1);
                    match self.poll_retry.next_wait(failures, &error, paused) {
                        Some(wait) => {
                            paused = paused.saturating_add(wait);
                            wait.max(interval)
                        }
                        None if Instant::now() >= deadline => {
                            return Err(self.give_up(took_too_long()));
                        }
                        None => return Err(self.give_up(tried(error, failures))),
                    }
                }
                Err(error) => return Err(error),
            };
            let now = Instant::now();
            std::thread::sleep(pause.min(deadline.saturating_duration_since(now)));
        }
    }

    /// Ends a wait that gave up: asks the provider to stop, records the give-up, and returns
    /// `error`.
    fn give_up(&self, error: AiError) -> AiError {
        let _ = self.cancel();
        if let Some(recorder) = &self.recorder {
            recorder.gave_up();
        }
        error
    }
}

/// The error for a wait that reached its timeout.
fn took_too_long() -> AiError {
    AiError::new(ErrorKind::Network, "the job took too long")
}
