//! One rule for trying a call again: how many times, how long to pause, and when to give up.
//!
//! Invariants:
//! - The caller decides which errors are worth another try. This module only decides the pause.
//!   A call that may already have been carried out (a job sent, then the connection dropped)
//!   must not be retried by its caller, or the person could pay twice.
//! - Each pause doubles, starting at `first_delay`, and is never shorter than the server's own
//!   `Retry-After` when the error carries one.
//! - A pause that would take the time spent past `budget` is not taken. The caller gives up and
//!   says so, rather than holding a worker for an hour because a server said `Retry-After: 3600`.
//! - `max_attempts` counts every try, the first included, and is clamped to 1..=5.

use std::time::{Duration, Instant};

use crate::error::AiError;

/// How a call is tried again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    /// Most tries, the first included. Clamped to 1..=5; 1 turns retries off.
    pub max_attempts: u8,
    /// Pause before the second try. Each later pause doubles.
    pub first_delay: Duration,
    /// The most time to spend from the first try to the last pause, in total.
    pub budget: Duration,
}

impl Backoff {
    /// Three tries, a one-second first pause, thirty seconds at most.
    pub const STANDARD: Self = Self {
        max_attempts: 3,
        first_delay: Duration::from_secs(1),
        budget: Duration::from_secs(30),
    };

    /// No retries: the first answer is the answer.
    pub const NEVER: Self = Self {
        max_attempts: 1,
        first_delay: Duration::ZERO,
        budget: Duration::ZERO,
    };

    /// The pause before the next try, after try number `attempt` (counting from 1) failed with
    /// `error` and `elapsed` has passed since the first try began. `None` means give up: the
    /// tries are used up, or the pause would go past the budget.
    pub fn next_wait(&self, attempt: u8, error: &AiError, elapsed: Duration) -> Option<Duration> {
        if attempt >= self.max_attempts.clamp(1, 5) {
            return None;
        }
        let doublings = u32::from(attempt.saturating_sub(1)).min(16);
        let backoff = self.first_delay.saturating_mul(1 << doublings);
        let wait = error
            .retry_after_secs
            .map_or(backoff, |secs| backoff.max(Duration::from_secs(secs)));
        (elapsed.saturating_add(wait) <= self.budget).then_some(wait)
    }

    /// Runs `call` until it succeeds, fails with an error `worth_retrying` refuses, or the
    /// tries or budget run out. `sleep` takes each pause: `std::thread::sleep` in a worker, a
    /// recorder in a test. A final error after more than one try says how many were made.
    pub fn run<T>(
        &self,
        worth_retrying: impl Fn(&AiError) -> bool,
        mut call: impl FnMut() -> Result<T, AiError>,
        mut sleep: impl FnMut(Duration),
    ) -> Result<T, AiError> {
        let started = Instant::now();
        let mut attempt = 1u8;
        loop {
            let error = match call() {
                Ok(value) => return Ok(value),
                Err(error) => error,
            };
            let wait = if worth_retrying(&error) {
                self.next_wait(attempt, &error, started.elapsed())
            } else {
                None
            };
            let Some(wait) = wait else {
                return Err(tried(error, attempt));
            };
            sleep(wait);
            attempt = attempt.saturating_add(1);
        }
    }
}

impl Default for Backoff {
    /// [`Backoff::STANDARD`].
    fn default() -> Self {
        Self::STANDARD
    }
}

/// The final error, saying how many tries were made when there was more than one. The kind and
/// any `Retry-After` are kept, so an app can still act on them.
pub fn tried(error: AiError, attempts: u8) -> AiError {
    if attempts <= 1 {
        return error;
    }
    let retry_after = error.retry_after_secs;
    let error = AiError::new(
        error.kind,
        format!("{} (after {attempts} tries)", error.message),
    );
    match retry_after {
        Some(secs) => error.with_retry_after(secs),
        None => error,
    }
}

#[cfg(test)]
#[path = "retry_tests.rs"]
mod tests;
