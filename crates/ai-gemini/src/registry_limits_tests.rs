//! The age and byte limits on uncollected Gemini results, without a live service.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use registry::{MAX_HELD_BYTES, UNCOLLECTED_TTL, admit};

/// A finished slot that ended `age` before `now`, holding one uncollected picture of `bytes`.
/// A handle carries its size without the bytes, so the byte limit is tested without 512 MB.
fn finished(now: Instant, age: Duration, bytes: u64) -> Slot {
    let mut slot = Slot::running();
    slot.events.push(Event::Output(Output {
        payload: Payload::handle("scripted://picture", "image/png", Some(bytes)).unwrap(),
        label: None,
    }));
    slot.finish(JobState::Succeeded, now - age);
    slot
}

fn id(n: usize) -> JobId {
    JobId::new(format!("job-{n}")).unwrap()
}

/// Expired uncollected results are forgotten even when the count is far from full.
#[test]
fn an_uncollected_result_is_forgotten_after_its_age_limit() {
    let now = Instant::now() + UNCOLLECTED_TTL * 2;
    let mut jobs = HashMap::from([
        (id(0), finished(now, UNCOLLECTED_TTL, 1)),
        (
            id(1),
            finished(now, UNCOLLECTED_TTL - Duration::from_secs(1), 1),
        ),
        (id(2), Slot::running()),
    ]);
    admit(&mut jobs, now).unwrap();
    assert!(!jobs.contains_key(&id(0)), "expired result kept");
    assert!(jobs.contains_key(&id(1)), "young result dropped");
    assert!(jobs.contains_key(&id(2)), "running job dropped");
}

/// A running job is never aged out, however long it has run.
#[test]
fn a_running_job_never_expires() {
    let now = Instant::now() + UNCOLLECTED_TTL * 4;
    let mut jobs = HashMap::from([(id(0), Slot::running())]);
    admit(&mut jobs, now).unwrap();
    assert!(jobs.contains_key(&id(0)));
}

/// New fills are refused while uncollected pictures hold the byte limit, and allowed below it.
#[test]
fn held_picture_bytes_refuse_new_fills() {
    let now = Instant::now();
    let half = MAX_HELD_BYTES / 2;
    let mut jobs = HashMap::from([
        (id(0), finished(now, Duration::ZERO, half)),
        (id(1), finished(now, Duration::ZERO, half - 1)),
    ]);
    admit(&mut jobs, now).unwrap();
    jobs.insert(id(2), finished(now, Duration::ZERO, 1));
    let error = admit(&mut jobs, now).unwrap_err();
    assert_eq!(error.kind, ErrorKind::RateLimited);
    assert_eq!(jobs.len(), 3, "a byte refusal must not forget results");
    // Collecting one picture frees its bytes.
    jobs.get_mut(&id(0)).unwrap().events.clear();
    admit(&mut jobs, now).unwrap();
}

/// Cancelling records when the job ended, so a cancelled job ages out too.
#[test]
fn cancelling_starts_the_age_clock() {
    let p = GeminiProvider::new(GeminiConfig::new(
        "http://127.0.0.1:9/v1beta",
        SecretString::new("scripted-test-key"),
    ))
    .unwrap();
    lock(&p.jobs).unwrap().insert(id(0), Slot::running());
    p.cancel(&id(0)).unwrap();
    let jobs = lock(&p.jobs).unwrap();
    assert!(jobs.get(&id(0)).unwrap().ended.is_some());
}
