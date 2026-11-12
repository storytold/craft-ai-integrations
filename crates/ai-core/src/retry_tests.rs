use super::*;
use crate::error::ErrorKind;
use std::cell::{Cell, RefCell};

/// A rate-limited error, the usual retryable failure.
fn busy() -> AiError {
    AiError::new(ErrorKind::RateLimited, "busy")
}

/// A rule with a short first pause and a long budget, so only the tries limit it.
fn quick(max_attempts: u8) -> Backoff {
    Backoff {
        max_attempts,
        first_delay: Duration::from_millis(10),
        budget: Duration::from_secs(60),
    }
}

/// Each pause doubles from the first delay.
#[test]
fn pauses_double() {
    let b = quick(5);
    let e = busy();
    assert_eq!(
        b.next_wait(1, &e, Duration::ZERO),
        Some(Duration::from_millis(10))
    );
    assert_eq!(
        b.next_wait(2, &e, Duration::ZERO),
        Some(Duration::from_millis(20))
    );
    assert_eq!(
        b.next_wait(3, &e, Duration::ZERO),
        Some(Duration::from_millis(40))
    );
}

/// The server's Retry-After wins when it is longer than the doubling pause, and not otherwise.
#[test]
fn retry_after_is_a_floor() {
    let b = quick(3);
    let e = busy().with_retry_after(2);
    assert_eq!(
        b.next_wait(1, &e, Duration::ZERO),
        Some(Duration::from_secs(2))
    );
    let slow = Backoff {
        first_delay: Duration::from_secs(5),
        ..b
    };
    assert_eq!(
        slow.next_wait(1, &e, Duration::ZERO),
        Some(Duration::from_secs(5))
    );
}

/// No pause after the last allowed try.
#[test]
fn stops_after_the_last_try() {
    let b = quick(3);
    assert!(b.next_wait(2, &busy(), Duration::ZERO).is_some());
    assert_eq!(b.next_wait(3, &busy(), Duration::ZERO), None);
}

/// A pause that would go past the budget is not taken; one that lands exactly on it is.
#[test]
fn the_budget_caps_the_pause() {
    let b = Backoff {
        budget: Duration::from_secs(30),
        ..quick(3)
    };
    let e = busy().with_retry_after(3600);
    assert_eq!(b.next_wait(1, &e, Duration::ZERO), None);
    let e = busy().with_retry_after(10);
    assert_eq!(
        b.next_wait(1, &e, Duration::from_secs(20)),
        Some(Duration::from_secs(10))
    );
    assert_eq!(b.next_wait(1, &e, Duration::from_secs(21)), None);
}

/// Attempts are clamped to 1..=5: zero behaves as one, and ten as five.
#[test]
fn attempts_are_clamped() {
    assert_eq!(quick(0).next_wait(1, &busy(), Duration::ZERO), None);
    assert!(quick(10).next_wait(4, &busy(), Duration::ZERO).is_some());
    assert_eq!(quick(10).next_wait(5, &busy(), Duration::ZERO), None);
}

/// NEVER gives the first answer back unchanged.
#[test]
fn never_does_not_retry() {
    let calls = Cell::new(0);
    let out: Result<(), _> = Backoff::NEVER.run(
        |_| true,
        || {
            calls.set(calls.get() + 1);
            Err(busy())
        },
        |_| panic!("no pause expected"),
    );
    assert_eq!(calls.get(), 1);
    assert_eq!(out.unwrap_err().message, "busy");
}

/// A call that fails twice then succeeds returns the value after two pauses.
#[test]
fn run_retries_until_success() {
    let calls = Cell::new(0);
    let pauses = RefCell::new(Vec::new());
    let out = quick(3).run(
        |e| e.kind == ErrorKind::RateLimited,
        || {
            calls.set(calls.get() + 1);
            if calls.get() < 3 { Err(busy()) } else { Ok(7) }
        },
        |d| pauses.borrow_mut().push(d),
    );
    assert_eq!(out.unwrap(), 7);
    assert_eq!(
        *pauses.borrow(),
        vec![Duration::from_millis(10), Duration::from_millis(20)]
    );
}

/// An error the caller will not retry is returned at once, with no try count added.
#[test]
fn run_returns_a_refused_error_at_once() {
    let calls = Cell::new(0);
    let out: Result<(), _> = quick(3).run(
        |e| e.kind == ErrorKind::RateLimited,
        || {
            calls.set(calls.get() + 1);
            Err(AiError::new(ErrorKind::Network, "dropped"))
        },
        |_| panic!("no pause expected"),
    );
    assert_eq!(calls.get(), 1);
    assert_eq!(out.unwrap_err().message, "dropped");
}

/// When the tries run out the final error keeps its kind and Retry-After and names the count.
#[test]
fn run_reports_the_tries_made() {
    let out: Result<(), _> = quick(3).run(|_| true, || Err(busy().with_retry_after(0)), |_| {});
    let e = out.unwrap_err();
    assert_eq!(e.kind, ErrorKind::RateLimited);
    assert_eq!(e.retry_after_secs, Some(0));
    assert_eq!(e.message, "busy (after 3 tries)");
}

/// A condition that changes during the pause (a cancel) stops the next try.
#[test]
fn run_asks_again_after_the_pause() {
    let calls = Cell::new(0);
    let cancelled = Cell::new(false);
    let out: Result<(), _> = quick(3).run(
        |_| !cancelled.get(),
        || {
            calls.set(calls.get() + 1);
            Err(busy())
        },
        |_| cancelled.set(true),
    );
    assert_eq!(calls.get(), 1);
    assert_eq!(out.unwrap_err().message, "busy");
}

/// The budget counts the pauses taken: one-second waits within a 2.5 s budget allow two pauses,
/// although each alone would fit.
#[test]
fn run_budget_counts_the_pauses() {
    let calls = Cell::new(0);
    let pauses = RefCell::new(Vec::new());
    let rule = Backoff {
        max_attempts: 5,
        first_delay: Duration::ZERO,
        budget: Duration::from_millis(2500),
    };
    let out: Result<(), _> = rule.run(
        |_| true,
        || {
            calls.set(calls.get() + 1);
            Err(busy().with_retry_after(1))
        },
        |d| pauses.borrow_mut().push(d),
    );
    assert!(out.is_err());
    assert_eq!(*pauses.borrow(), vec![Duration::from_secs(1); 2]);
    assert_eq!(calls.get(), 3);
}

/// A slow call that fails does not use up the budget: the time spent calling is not a pause.
#[test]
fn run_slow_failure_is_still_retried() {
    let calls = Cell::new(0);
    let rule = Backoff {
        max_attempts: 3,
        first_delay: Duration::from_millis(10),
        budget: Duration::from_millis(20),
    };
    let out = rule.run(
        |_| true,
        || {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                std::thread::sleep(Duration::from_millis(40));
                Err(busy())
            } else {
                Ok(())
            }
        },
        |_| {},
    );
    assert!(out.is_ok());
    assert_eq!(calls.get(), 2);
}
