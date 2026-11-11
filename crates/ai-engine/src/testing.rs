#![allow(clippy::expect_used, clippy::panic)]
//! For an engine crate's own tests: a runner that behaves as scripted, and the checks any runner
//! must pass. `check_runner` puts a runner behind the real provider and drives it through the
//! shared provider conformance suite, so a runner that passes is one every app can use.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use craft_ai_core::conformance::{
    run_cancel_paced, run_lifecycle_paced, run_rejects, run_unknown_job,
};
use craft_ai_core::{
    AiError, CapabilityId, CapabilitySpec, ErrorKind, InputRole, JobState, ModelId, ModelInfo,
    Output, Payload, Provider, Request,
};

use craft_ai_features::GpuVendor;

use crate::provider::EngineProvider;
use crate::runner::{LoadedModel, ModelRunner, RunContext};

/// What a [`FakeRunner`] does when it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behaviour {
    Succeed,
    /// Fails with a provider error after its steps.
    Fail,
    /// Panics part way, to prove a bad runner cannot take the app down.
    Panic,
}

/// A model that takes a `source` input, works for a few short steps and answers with one text
/// output. Counts how many times it was loaded.
#[derive(Clone)]
pub struct FakeRunner {
    pub engine: String,
    pub model: String,
    pub capability: String,
    pub steps: u32,
    pub step_pause: Duration,
    pub behaviour: Behaviour,
    pub loads: Arc<AtomicUsize>,
    /// How many times `run` was entered (a job that never started is not counted).
    pub runs: Arc<AtomicUsize>,
    pub vendors: Vec<GpuVendor>,
    /// Never looks at Cancel, like a runner that forgot to.
    pub deaf: bool,
    /// What the loaded model says it holds, read each time it is asked, so a test can change
    /// it between runs. `None` leaves the provider to estimate.
    pub memory: Option<Arc<AtomicU64>>,
    /// While set, a run waits after its first step (still answering Cancel), so a test can look
    /// at a job that is certainly running. Clear it to let the run finish.
    pub hold: Option<Arc<AtomicBool>>,
    /// While set, a load waits before reading anything, so a test can act on a job that has
    /// not loaded its model yet. Clear it to let the load go on.
    pub load_hold: Option<Arc<AtomicBool>>,
    /// How many loads have started (counted before `load_hold` is waited on).
    pub load_started: Arc<AtomicUsize>,
}

impl FakeRunner {
    pub fn new(engine: &str, model: &str, capability: &str) -> Self {
        Self {
            engine: engine.to_owned(),
            model: model.to_owned(),
            capability: capability.to_owned(),
            steps: 4,
            step_pause: Duration::from_millis(5),
            behaviour: Behaviour::Succeed,
            loads: Arc::default(),
            runs: Arc::default(),
            vendors: Vec::new(),
            deaf: false,
            memory: None,
            hold: None,
            load_hold: None,
            load_started: Arc::default(),
        }
    }

    pub fn behaving(mut self, behaviour: Behaviour) -> Self {
        self.behaviour = behaviour;
        self
    }

    /// A request this runner accepts.
    pub fn good_request(&self) -> Result<Request, AiError> {
        Ok(Request::new(parse_cap(&self.capability)?)
            .with_input(InputRole::Source, Payload::text("a picture")))
    }

    /// A request this runner must refuse: it has no source.
    pub fn bad_request(&self) -> Result<Request, AiError> {
        Ok(Request::new(parse_cap(&self.capability)?))
    }
}

fn parse_cap(text: &str) -> Result<CapabilityId, AiError> {
    CapabilityId::parse(text).map_err(|e| AiError::invalid(e.to_string()))
}

impl ModelRunner for FakeRunner {
    fn engine(&self) -> &str {
        &self.engine
    }

    fn model(&self) -> ModelInfo {
        let id = ModelId::new(&self.model).expect("a valid fake model id");
        let capability = parse_cap(&self.capability).expect("a valid fake capability");
        ModelInfo::new(id, "Fake model").with(
            CapabilitySpec::new(capability)
                .accepting(InputRole::Source, 1)
                .requiring(InputRole::Source),
        )
    }

    fn accelerators(&self) -> &[GpuVendor] {
        &self.vendors
    }

    fn load(&self, weights: &Path) -> Result<Box<dyn LoadedModel>, AiError> {
        if !weights.is_file() {
            return Err(AiError::unsupported("no such weights"));
        }
        self.load_started.fetch_add(1, Ordering::SeqCst);
        while self
            .load_hold
            .as_ref()
            .is_some_and(|h| h.load(Ordering::SeqCst))
        {
            std::thread::sleep(Duration::from_millis(1));
        }
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(self.clone()))
    }
}

impl LoadedModel for FakeRunner {
    fn run(&mut self, _request: &Request, ctx: &RunContext) -> Result<Vec<Output>, AiError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        while self.hold.as_ref().is_some_and(|h| h.load(Ordering::SeqCst)) {
            ctx.check_cancelled()?;
            std::thread::sleep(Duration::from_millis(1));
        }
        for step in 0..self.steps {
            if !self.deaf {
                ctx.check_cancelled()?;
            }
            if self.behaviour == Behaviour::Panic && step == 1 {
                panic!("the fake runner panicked on purpose");
            }
            ctx.progress(u8::try_from(step * 100 / self.steps.max(1)).unwrap_or(99));
            std::thread::sleep(self.step_pause);
        }
        if self.behaviour == Behaviour::Fail {
            return Err(AiError::new(ErrorKind::Provider, "the fake runner failed"));
        }
        Ok(vec![Output {
            payload: Payload::text("done"),
            label: None,
        }])
    }

    fn memory_bytes(&self) -> Option<u64> {
        self.memory.as_ref().map(|m| m.load(Ordering::SeqCst))
    }
}

/// Puts `runner` behind the real provider with `weights`, and runs every shared check: the whole
/// life of a job, Cancel, a refused request, an unknown job, missing weights, a second job after
/// the first (weights reused) and after a cancel (the runner not left stuck). Returns the first
/// thing it got wrong, in words.
pub fn check_runner(
    runner: Arc<dyn ModelRunner>,
    weights: &Path,
    good: &Request,
    bad: &Request,
    bad_kind: ErrorKind,
) -> Result<(), String> {
    check_runner_paced(
        runner,
        weights,
        good,
        bad,
        bad_kind,
        Duration::from_millis(3),
    )
}

/// As [`check_runner`], waiting `pause` between polls. The shared suite allows a job a thousand
/// polls, so a real model that takes half a minute needs a pause of 50 ms or more.
pub fn check_runner_paced(
    runner: Arc<dyn ModelRunner>,
    weights: &Path,
    good: &Request,
    bad: &Request,
    bad_kind: ErrorKind,
    pause: Duration,
) -> Result<(), String> {
    let model = runner.model().id;
    let provider = EngineProvider::new("conformance", [(runner.clone(), weights.to_path_buf())]);
    let pause = || std::thread::sleep(pause);

    let done = run_lifecycle_paced(&provider, &model, good, &pause)?;
    if done.state != JobState::Succeeded {
        return Err(format!("a good request ended as {:?}", done.state));
    }
    if !done
        .events
        .iter()
        .any(|e| matches!(e, craft_ai_core::Event::Output(_)))
    {
        return Err("a succeeded job produced no output".into());
    }
    run_cancel_paced(&provider, &model, good, &pause)?;
    run_rejects(&provider, &model, bad, bad_kind)?;
    run_unknown_job(&provider)?;

    let again = run_lifecycle_paced(&provider, &model, good, &pause)?;
    if again.state != JobState::Succeeded {
        return Err(format!("a job after a cancel ended as {:?}", again.state));
    }

    let missing = weights.with_file_name("not-there.safetensors");
    let lost = EngineProvider::new("conformance", [(runner, missing)]);
    run_rejects(&lost, &model, good, ErrorKind::Unsupported)
        .map_err(|e| format!("missing weights: {e}"))?;
    let listed = provider.models().map_err(|e| e.to_string())?;
    if listed.iter().any(|m| m.local_file.is_none()) {
        return Err("a listed model does not carry its weights file".into());
    }
    Ok(())
}
