//! Models the built-in engines keep in memory, through the app's client: listing them with their
//! connection, freeing one, all or the idle ones, and refusing a load that will not fit.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use craft_ai_client::{AiClient, EngineRegistry, FreeMemory, ProviderFactory, ToolDesk};
use craft_ai_core::{
    AppId, ConnectionId, ConsentLedger, InputRole, JobState, ModelId, Payload, Request, StoreFile,
};
use craft_ai_engine::testing::FakeRunner;
use craft_ai_features::Feature;
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;

fn select_subject() -> Feature {
    craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap()
}

struct World {
    dir: tempfile::TempDir,
    shared: SharedStore,
}

/// A factory whose only engines are the ones given, whatever cargo features are on.
fn factory(registry: EngineRegistry) -> ProviderFactory {
    let mut factory = ProviderFactory::standard();
    for built_in in ["rten", "candle", "burn"] {
        factory.unregister(built_in);
    }
    factory.register_engines(registry);
    factory
}

fn registry(runner: &FakeRunner) -> EngineRegistry {
    let mut registry = EngineRegistry::new();
    registry.register(runner.clone());
    registry
}

impl World {
    /// A shared store with select-subject set up on Candle from a weights file of `size` bytes.
    fn new(size: usize) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let shared = SharedStore::new(dir.path().join("connections.json"));
        shared.replace(&StoreFile::default()).unwrap();
        let w = Self { dir, shared };
        let file = w.dir.path().join("sam3.safetensors");
        std::fs::write(&file, vec![0u8; size]).unwrap();
        let desk = ToolDesk::new(w.bare("launcher"), w.shared.clone(), None);
        desk.enable_native_with(&select_subject(), "candle", &file)
            .unwrap();
        w
    }

    fn bare(&self, app: &str) -> AiClient {
        AiClient::new(
            AppId::new(app).unwrap(),
            self.shared.clone(),
            Arc::new(MemoryStore::new()),
        )
    }

    fn client(&self, runner: &FakeRunner) -> AiClient {
        self.bare("lightcraft")
            .with_factory(factory(registry(runner)))
    }
}

fn start(app: &AiClient) -> craft_ai_client::Job {
    let role = select_subject().role().unwrap();
    let request = Request::new(select_subject().capability.clone())
        .with_input(InputRole::Source, Payload::text("the picture"));
    let plan = app.plan(&role, request).unwrap();
    app.start(&plan, &ConsentLedger::default()).unwrap()
}

fn run(app: &AiClient) -> JobState {
    start(app)
        .wait(Duration::from_millis(2), Duration::from_secs(10))
        .unwrap()
        .state
}

fn until(what: &str, check: impl Fn() -> bool) {
    let end = std::time::Instant::now() + Duration::from_secs(10);
    while !check() {
        assert!(std::time::Instant::now() < end, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn runner() -> FakeRunner {
    FakeRunner::new("candle", "sam3", "image.segment")
}

fn sam3() -> ModelId {
    ModelId::new("sam3").unwrap()
}

fn reading(bytes: Option<u64>) -> FreeMemory {
    Arc::new(move || bytes)
}

#[test]
fn a_loaded_model_is_listed_with_its_engine_and_connection() {
    let w = World::new(300);
    let r = runner();
    let app = w.client(&r);
    assert!(app.loaded_models().is_empty(), "nothing loads before a job");

    assert_eq!(run(&app), JobState::Succeeded);
    let loaded = app.loaded_models();
    assert_eq!(loaded.len(), 1);
    let saved = w.shared.load().unwrap();
    let connection = saved
        .connections
        .iter()
        .find(|c| c.id == loaded[0].connection)
        .expect("listed under the saved connection");
    assert_eq!(loaded[0].connection_label, connection.label);
    assert_eq!(loaded[0].engine, "candle");
    assert_eq!(loaded[0].model.model, sam3());
    assert_eq!(loaded[0].model.bytes, 300);
    assert!(loaded[0].model.estimated);
}

#[test]
fn unloading_all_frees_the_model_and_the_next_job_loads_it_again() {
    let w = World::new(3);
    let r = runner();
    let loads = r.loads.clone();
    let app = w.client(&r);
    run(&app);

    assert_eq!(app.unload_all_models().freed, vec![sam3()]);
    assert!(app.loaded_models().is_empty());
    assert_eq!(run(&app), JobState::Succeeded);
    assert_eq!(loads.load(Ordering::SeqCst), 2);
}

#[test]
fn unloading_one_model_needs_its_connection() {
    let w = World::new(3);
    let r = runner();
    let app = w.client(&r);
    run(&app);
    let connection = app.loaded_models()[0].connection.clone();

    let other = ConnectionId::new("not-this-one").unwrap();
    assert_eq!(app.unload_model(&other, &sam3()), Default::default());
    assert_eq!(
        app.unload_model(&connection, &ModelId::new("other").unwrap()),
        Default::default()
    );
    assert_eq!(app.loaded_models().len(), 1);
    assert_eq!(app.unload_model(&connection, &sam3()).freed, vec![sam3()]);
    assert!(app.loaded_models().is_empty());
}

#[test]
fn idle_unloading_frees_only_models_rested_long_enough() {
    let w = World::new(3);
    let r = runner();
    let app = w.client(&r);
    run(&app);

    assert_eq!(
        app.unload_idle_models(Duration::from_secs(3600)),
        Default::default()
    );
    assert_eq!(app.loaded_models().len(), 1);
    assert_eq!(app.unload_idle_models(Duration::ZERO).freed, vec![sam3()]);
}

fn refused(state: JobState) -> bool {
    matches!(state, JobState::Failed { error } if error.message.contains("enough free memory"))
}

#[test]
fn a_load_that_will_not_fit_is_refused_whichever_order_the_client_is_built_in() {
    let w = World::new(2_000);
    for reader_first in [true, false] {
        let r = runner();
        let loads = r.loads.clone();
        let base = w.bare("lightcraft");
        let app = if reader_first {
            base.with_free_memory(reading(Some(1_000)))
                .with_factory(factory(registry(&r)))
        } else {
            base.with_factory(factory(registry(&r)))
                .with_free_memory(reading(Some(1_000)))
        };
        assert!(refused(run(&app)), "reader first: {reader_first}");
        assert_eq!(loads.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn a_reader_set_on_the_factory_before_its_engines_still_applies() {
    let w = World::new(2_000);
    let r = runner();
    let mut f = ProviderFactory::standard();
    for built_in in ["rten", "candle", "burn"] {
        f.unregister(built_in);
    }
    f.set_free_memory(Some(reading(Some(1_000))));
    f.register_engines(registry(&r));
    let app = w.bare("lightcraft").with_factory(f);
    assert!(refused(run(&app)));
}

/// The provider was built before the reader was set; its next load still checks.
#[test]
fn a_reader_set_after_a_model_loaded_applies_to_the_next_load() {
    let w = World::new(2_000);
    let r = runner();
    let mut f = factory(registry(&r));
    let app = w.bare("lightcraft").with_factory(f.clone());
    assert_eq!(run(&app), JobState::Succeeded);

    f.set_free_memory(Some(reading(Some(1_000))));
    let app = w.bare("lightcraft").with_factory(f);
    assert_eq!(run(&app), JobState::Succeeded, "already loaded: no check");
    app.unload_all_models();
    assert!(refused(run(&app)));
}

#[test]
fn replacing_an_engine_forgets_the_models_its_old_adapter_loaded() {
    let w = World::new(3);
    let r = runner();
    let mut f = factory(registry(&r));
    let app = w.bare("lightcraft").with_factory(f.clone());
    run(&app);
    assert_eq!(app.loaded_models().len(), 1);

    f.unregister("candle");
    let app = w.bare("lightcraft").with_factory(f);
    assert!(app.loaded_models().is_empty());
}

/// The tool is set up again with a new file while a job still runs on the old one. The old model
/// stays listed while that job runs, and goes as it finishes.
#[test]
fn a_model_whose_connection_changed_mid_job_stays_listed_until_the_job_ends() {
    let w = World::new(3);
    let mut r = runner();
    let hold = Arc::new(AtomicBool::new(true));
    r.hold = Some(hold.clone());
    let runs = r.runs.clone();
    let app = w.client(&r);
    let first = start(&app);
    until("the first job is running", || {
        runs.load(Ordering::SeqCst) == 1
    });

    let file = w.dir.path().join("sam3-new.safetensors");
    std::fs::write(&file, b"new").unwrap();
    let desk = ToolDesk::new(w.bare("launcher"), w.shared.clone(), None);
    desk.enable_native_with(&select_subject(), "candle", &file)
        .unwrap();
    let second = start(&app);
    until("the second job is running", || {
        runs.load(Ordering::SeqCst) == 2
    });
    assert_eq!(
        app.loaded_models().len(),
        2,
        "the old model is still in sight"
    );

    hold.store(false, Ordering::SeqCst);
    for job in [first, second] {
        let done = job
            .wait(Duration::from_millis(2), Duration::from_secs(10))
            .unwrap();
        assert_eq!(done.state, JobState::Succeeded);
    }
    until("the old model is let go of", || {
        app.loaded_models().len() == 1
    });
    assert_eq!(
        app.loaded_models()[0].model.bytes,
        3,
        "the new file's model remains"
    );
}

/// Clients built from one factory share one loaded copy of a model, not two.
#[test]
fn clients_from_one_factory_share_one_loaded_copy() {
    let w = World::new(3);
    let r = runner();
    let loads = r.loads.clone();
    let f = factory(registry(&r));
    let one = w.bare("lightcraft").with_factory(f.clone());
    let two = w.bare("photocraft").with_factory(f);
    run(&one);
    run(&two);
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    let ids = |c: &AiClient| -> Vec<_> {
        c.loaded_models()
            .into_iter()
            .map(|m| (m.connection, m.model.model))
            .collect()
    };
    assert_eq!(ids(&one).len(), 1);
    assert_eq!(ids(&one), ids(&two));
}
