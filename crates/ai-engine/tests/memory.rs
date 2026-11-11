//! Models kept in memory, through the real provider: listing, unloading, idle unloading and the
//! free-memory check before a load.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use craft_ai_core::{ErrorKind, JobId, JobState, ModelId, Provider};
use craft_ai_engine::testing::FakeRunner;
use craft_ai_engine::{EngineProvider, FreeMemory, ModelRunner};

/// A weights file of `size` bytes, so the estimate is checkable.
fn weights(dir: &tempfile::TempDir, name: &str, size: usize) -> std::path::PathBuf {
    let p = dir.path().join(name);
    std::fs::write(&p, vec![0u8; size]).unwrap();
    p
}

fn id(model: &str) -> ModelId {
    ModelId::new(model).unwrap()
}

fn provider(runners: Vec<(FakeRunner, std::path::PathBuf)>) -> EngineProvider {
    EngineProvider::new(
        "candle",
        runners
            .into_iter()
            .map(|(r, w)| (Arc::new(r) as Arc<dyn ModelRunner>, w)),
    )
}

fn finish(p: &dyn Provider, job: &JobId) -> JobState {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let snap = p.poll(job).unwrap();
        if snap.state.is_terminal() {
            return snap.state;
        }
        assert!(Instant::now() < end, "job never finished");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn run(p: &EngineProvider, r: &FakeRunner) -> JobState {
    let job = p.submit(&id(&r.model), &r.good_request().unwrap()).unwrap();
    finish(p, &job)
}

/// Makes `r` wait inside its run until the returned flag is cleared.
fn held(r: &mut FakeRunner) -> Arc<AtomicBool> {
    let hold = Arc::new(AtomicBool::new(true));
    r.hold = Some(hold.clone());
    hold
}

fn until(what: &str, check: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(Instant::now() < end, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_model_is_listed_once_loaded_with_the_file_size_as_its_estimate() {
    let dir = tempfile::tempdir().unwrap();
    let r = FakeRunner::new("candle", "sam3", "image.segment");
    let p = provider(vec![(r.clone(), weights(&dir, "m.bin", 300))]);
    assert!(p.loaded_models().is_empty(), "nothing loads before a job");

    assert_eq!(run(&p, &r), JobState::Succeeded);
    let loaded = p.loaded_models();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].model, id("sam3"));
    assert_eq!(loaded[0].label, "Fake model");
    assert_eq!(loaded[0].bytes, 300);
    assert!(loaded[0].estimated);
    assert!(!loaded[0].running);
}

/// The figure a model reports is shown from the moment it loads, during its first run.
#[test]
fn a_model_that_reports_its_memory_is_listed_with_that_figure_while_it_runs() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = FakeRunner::new("candle", "sam3", "image.segment");
    let hold = held(&mut r);
    r.memory = Some(Arc::new(AtomicU64::new(700_000_000)));
    let runs = r.runs.clone();
    let p = provider(vec![(r.clone(), weights(&dir, "m.bin", 3))]);
    let job = p.submit(&id("sam3"), &r.good_request().unwrap()).unwrap();
    until("the job is running", || runs.load(Ordering::SeqCst) == 1);
    let loaded = p.loaded_models();
    assert!(loaded[0].running);
    assert_eq!(loaded[0].bytes, 700_000_000);
    assert!(!loaded[0].estimated);
    hold.store(false, Ordering::SeqCst);
    finish(&p, &job);
}

/// A model that keeps work between runs (SAM keeps the picture) grows; the list follows it.
#[test]
fn a_model_that_grows_between_runs_is_listed_at_its_new_size() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = FakeRunner::new("candle", "sam3", "image.segment");
    let memory = Arc::new(AtomicU64::new(100));
    r.memory = Some(memory.clone());
    let p = provider(vec![(r.clone(), weights(&dir, "m.bin", 3))]);
    run(&p, &r);
    memory.store(250, Ordering::SeqCst);
    run(&p, &r);
    assert_eq!(p.loaded_models()[0].bytes, 250);
}

#[test]
fn unloading_frees_the_model_and_the_next_job_loads_it_again() {
    let dir = tempfile::tempdir().unwrap();
    let r = FakeRunner::new("candle", "sam3", "image.segment");
    let loads = r.loads.clone();
    let p = provider(vec![(r.clone(), weights(&dir, "m.bin", 3))]);
    run(&p, &r);

    let done = p.unload(None);
    assert_eq!(done.freed, vec![id("sam3")]);
    assert!(done.after_run.is_empty());
    assert!(p.loaded_models().is_empty());
    assert!(
        p.unload(None).freed.is_empty(),
        "a second unload has nothing to free"
    );

    assert_eq!(run(&p, &r), JobState::Succeeded);
    assert_eq!(loads.load(Ordering::SeqCst), 2, "loaded again after unload");
}

#[test]
fn unloading_one_model_leaves_the_others_loaded() {
    let dir = tempfile::tempdir().unwrap();
    let a = FakeRunner::new("candle", "sam3", "image.segment");
    let b = FakeRunner::new("candle", "other", "image.segment");
    let p = provider(vec![
        (a.clone(), weights(&dir, "a.bin", 3)),
        (b.clone(), weights(&dir, "b.bin", 3)),
    ]);
    run(&p, &a);
    run(&p, &b);

    assert_eq!(p.unload(Some(&id("other"))).freed, vec![id("other")]);
    let left: Vec<_> = p.loaded_models().into_iter().map(|m| m.model).collect();
    assert_eq!(left, vec![id("sam3")]);
    assert_eq!(p.unload(Some(&id("unknown"))), Default::default());
}

/// The model in use finishes its job, then goes. Nothing interrupts the run.
#[test]
fn a_model_in_use_is_unloaded_when_its_job_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = FakeRunner::new("candle", "sam3", "image.segment");
    let hold = held(&mut r);
    let (loads, runs) = (r.loads.clone(), r.runs.clone());
    let p = provider(vec![(r.clone(), weights(&dir, "m.bin", 3))]);
    let job = p.submit(&id("sam3"), &r.good_request().unwrap()).unwrap();
    until("the job is running", || runs.load(Ordering::SeqCst) == 1);
    assert!(p.loaded_models()[0].running);

    let done = p.unload(None);
    assert!(done.freed.is_empty());
    assert_eq!(done.after_run, vec![id("sam3")]);
    assert_eq!(p.loaded_models().len(), 1, "still loaded while it runs");
    hold.store(false, Ordering::SeqCst);
    assert_eq!(finish(&p, &job), JobState::Succeeded, "the run finished");
    until("the model is let go of", || p.loaded_models().is_empty());

    run(&p, &r);
    assert_eq!(loads.load(Ordering::SeqCst), 2);
    assert_eq!(
        p.loaded_models().len(),
        1,
        "the request was met, not carried over"
    );
}

#[test]
fn idle_unload_frees_only_models_rested_long_enough() {
    let dir = tempfile::tempdir().unwrap();
    let r = FakeRunner::new("candle", "sam3", "image.segment");
    let p = provider(vec![(r.clone(), weights(&dir, "m.bin", 3))]);
    run(&p, &r);

    assert_eq!(p.unload_idle(Duration::from_secs(3600)), Default::default());
    assert_eq!(p.loaded_models().len(), 1);
    assert_eq!(p.unload_idle(Duration::ZERO).freed, vec![id("sam3")]);
    assert!(p.loaded_models().is_empty());
}

#[test]
fn idle_unload_leaves_a_model_in_use_alone() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = FakeRunner::new("candle", "sam3", "image.segment");
    let hold = held(&mut r);
    let runs = r.runs.clone();
    let p = provider(vec![(r.clone(), weights(&dir, "m.bin", 3))]);
    let job = p.submit(&id("sam3"), &r.good_request().unwrap()).unwrap();
    until("the job is running", || runs.load(Ordering::SeqCst) == 1);

    assert_eq!(p.unload_idle(Duration::ZERO), Default::default());
    hold.store(false, Ordering::SeqCst);
    finish(&p, &job);
    assert_eq!(
        p.loaded_models().len(),
        1,
        "an idle sweep does not mark a busy model"
    );
}

fn reading(bytes: Option<u64>) -> FreeMemory {
    Arc::new(move || bytes)
}

#[test]
fn a_load_that_will_not_fit_is_refused_in_words_and_never_starts() {
    let dir = tempfile::tempdir().unwrap();
    let r = FakeRunner::new("candle", "sam3", "image.segment");
    let loads = r.loads.clone();
    let p = provider(vec![(r.clone(), weights(&dir, "m.bin", 2_000))])
        .with_free_memory(reading(Some(1_000)));

    match run(&p, &r) {
        JobState::Failed { error } => {
            assert_eq!(error.kind, ErrorKind::Unsupported);
            assert!(
                error.message.contains("enough free memory"),
                "{}",
                error.message
            );
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(loads.load(Ordering::SeqCst), 0, "nothing was loaded");
    assert!(p.loaded_models().is_empty());
}

#[test]
fn a_load_that_fits_or_cannot_be_judged_goes_ahead() {
    let dir = tempfile::tempdir().unwrap();
    for free in [Some(2_000), None] {
        let r = FakeRunner::new("candle", "sam3", "image.segment");
        let p = provider(vec![(r.clone(), weights(&dir, "m.bin", 2_000))])
            .with_free_memory(reading(free));
        assert_eq!(run(&p, &r), JobState::Succeeded, "free {free:?}");
    }
}

/// The check is for loads only: a model already in memory runs however little is free.
#[test]
fn a_loaded_model_runs_again_without_a_memory_check() {
    let dir = tempfile::tempdir().unwrap();
    let r = FakeRunner::new("candle", "sam3", "image.segment");
    let p =
        provider(vec![(r.clone(), weights(&dir, "m.bin", 2_000))]).with_free_memory(Arc::new({
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            move || (calls.fetch_add(1, Ordering::SeqCst) > 0).then_some(0)
        }));
    assert_eq!(run(&p, &r), JobState::Succeeded, "first load: unknown");
    assert_eq!(run(&p, &r), JobState::Succeeded, "already loaded");
}

/// The reader is the app's code. If it panics, that job fails in words and the next job runs.
#[test]
fn a_free_memory_reader_that_panics_fails_the_job_not_the_provider() {
    let dir = tempfile::tempdir().unwrap();
    let r = FakeRunner::new("candle", "sam3", "image.segment");
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let reader: FreeMemory = Arc::new({
        let calls = calls.clone();
        move || {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("the reader failed on purpose");
            }
            None
        }
    });
    let p = provider(vec![(r.clone(), weights(&dir, "m.bin", 3))]).with_free_memory(reader);
    match run(&p, &r) {
        JobState::Failed { error } => assert_eq!(error.kind, ErrorKind::Internal),
        other => panic!("expected a failed job, got {other:?}"),
    }
    assert!(p.loaded_models().is_empty());
    assert_eq!(run(&p, &r), JobState::Succeeded, "the next job still runs");
}
