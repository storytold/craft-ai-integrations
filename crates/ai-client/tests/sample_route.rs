//! The "try it" step picks the sample that suits the chosen route, and a selection that chose
//! nothing fails it. Scripted providers only; no model, service or network is used.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use craft_ai_client::{AiClient, ProviderFactory, SampleReport, TestStart, ToolDesk};
use craft_ai_core::job::{Event, Output};
use craft_ai_core::segment::{self, Found};
use craft_ai_core::{
    AiError, ConnectionId, InputRole, JobId, JobSnapshot, JobState, ModelId, ModelInfo, Payload,
    Provider, Request,
};
use craft_ai_features::{Feature, builtin};
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;

/// Answers every job at once with a mask and a [`Found`], and keeps the requests it was given.
struct Selector {
    matched: bool,
    seen: Arc<Mutex<Vec<Request>>>,
}

impl Provider for Selector {
    /// No model list; the connection carries it.
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        Ok(Vec::new())
    }
    /// Keeps the request so a test can read what the sample sent.
    fn submit(&self, _: &ModelId, request: &Request) -> Result<JobId, AiError> {
        self.seen.lock().unwrap().push(request.clone());
        JobId::new("job-1").map_err(|e| AiError::invalid(e.to_string()))
    }
    /// A finished job: a mask, then what the selection found.
    fn poll(&self, _: &JobId) -> Result<JobSnapshot, AiError> {
        let mut snap = JobSnapshot::new(JobState::Succeeded);
        snap.events.push(Event::Output(Output {
            payload: Payload::inline(segment::MASK_MEDIA_TYPE, vec![1, 2, 3])?,
            label: None,
        }));
        let found = Found {
            matched: self.matched,
            score: Some(0.9),
            name: None,
        };
        snap.events
            .push(Event::Output(segment::found_output(&found)?));
        Ok(snap)
    }
    /// Nothing to stop: the job is already done.
    fn cancel(&self, _: &JobId) -> Result<(), AiError> {
        Ok(())
    }
}

/// A desk whose factory answers for both routes with a [`Selector`], and the requests it sees.
fn desk(
    dir: &tempfile::TempDir,
    matched: bool,
) -> (ToolDesk, SharedStore, Arc<Mutex<Vec<Request>>>) {
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut factory = ProviderFactory::standard();
    for name in ["rten", "comfyui"] {
        let seen = Arc::clone(&seen);
        factory.register(name, move |_, _| {
            Ok(Arc::new(Selector {
                matched,
                seen: Arc::clone(&seen),
            }))
        });
    }
    let client = AiClient::new(
        ToolDesk::app().unwrap(),
        shared.clone(),
        Arc::new(MemoryStore::new()),
    )
    .with_factory(factory);
    (ToolDesk::new(client, shared.clone(), None), shared, seen)
}

/// The built-in Select subject tool.
fn select_subject() -> Feature {
    builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap()
}

/// Runs the sample to its end.
fn run(desk: &ToolDesk, feature: &Feature) -> SampleReport {
    let TestStart::Started(job) = desk.start_sample(feature, None).unwrap() else {
        panic!("a local sample needs no agreement");
    };
    ToolDesk::finish(&feature.role().unwrap(), &job, Duration::from_secs(5))
}

/// A built-in route is tried with one click and no words, which is what it reads.
#[test]
fn a_built_in_route_is_tried_with_a_click_and_no_description() {
    let dir = tempfile::tempdir().unwrap();
    let (desk, _, seen) = desk(&dir, true);
    let weights = dir.path().join("encoder.onnx");
    std::fs::write(&weights, b"x").unwrap();
    let f = select_subject();
    desk.enable_native_with(&f, "rten", &weights).unwrap();
    assert!(matches!(run(&desk, &f), SampleReport::Passed { .. }));
    let request = seen.lock().unwrap().pop().unwrap();
    let clicks = segment::clicks(&request).unwrap();
    assert!(clicks.len() == 1 && clicks[0].positive);
    assert!(segment::description(&request).is_none());
    assert!(request.inputs.iter().any(|i| i.role == InputRole::Source));
}

/// A route that is not built in (a ComfyUI workflow) is still tried with the description.
#[test]
fn a_comfyui_route_is_tried_with_the_description() {
    let dir = tempfile::tempdir().unwrap();
    let (desk, _, seen) = desk(&dir, true);
    let f = select_subject();
    desk.enable_local(&f, "http://127.0.0.1:8188").unwrap();
    assert!(matches!(run(&desk, &f), SampleReport::Passed { .. }));
    let request = seen.lock().unwrap().pop().unwrap();
    assert!(segment::description(&request).is_some());
    assert!(segment::clicks(&request).unwrap().is_empty());
}

/// A selection that ran but chose nothing fails the test and says so.
#[test]
fn a_selection_that_chose_nothing_fails_the_test() {
    let dir = tempfile::tempdir().unwrap();
    let (desk, _, _) = desk(&dir, false);
    let weights = dir.path().join("encoder.onnx");
    std::fs::write(&weights, b"x").unwrap();
    let f = select_subject();
    desk.enable_native_with(&f, "rten", &weights).unwrap();
    match run(&desk, &f) {
        SampleReport::Failed { message } => assert!(message.contains("selected nothing")),
        other => panic!("expected a failed test, got {other:?}"),
    }
}

/// A custom connection that only reuses the built-in engine's name is tried with the description,
/// since its model may need words; only the connection this suite saves gets the click.
#[test]
fn a_custom_connection_with_a_built_in_engine_name_gets_the_description() {
    let dir = tempfile::tempdir().unwrap();
    let (desk, shared, seen) = desk(&dir, true);
    let weights = dir.path().join("encoder.onnx");
    std::fs::write(&weights, b"x").unwrap();
    let f = select_subject();
    desk.enable_native_with(&f, "rten", &weights).unwrap();
    let custom = ConnectionId::new("my-own-rten").unwrap();
    shared
        .update(|store| {
            for connection in &mut store.connections {
                connection.id = custom.clone();
            }
            for binding in &mut store.defaults {
                binding.connection = custom.clone();
            }
        })
        .unwrap();
    assert!(matches!(run(&desk, &f), SampleReport::Passed { .. }));
    let request = seen.lock().unwrap().pop().unwrap();
    assert!(segment::description(&request).is_some());
    assert!(segment::clicks(&request).unwrap().is_empty());
}

/// Connection management can put another model under the built-in connection's id. Only the model
/// the engine's recipe names gets the click; another one may need words.
#[test]
fn another_model_under_the_built_in_connection_gets_the_description() {
    let dir = tempfile::tempdir().unwrap();
    let (desk, shared, seen) = desk(&dir, true);
    let weights = dir.path().join("encoder.onnx");
    std::fs::write(&weights, b"x").unwrap();
    let f = select_subject();
    desk.enable_native_with(&f, "rten", &weights).unwrap();
    let swapped = ModelId::new("sam-2.1-fp16").unwrap();
    shared
        .update(|store| {
            for connection in &mut store.connections {
                for model in &mut connection.models {
                    model.id = swapped.clone();
                }
            }
            for binding in &mut store.defaults {
                binding.model = swapped.clone();
            }
        })
        .unwrap();
    assert!(matches!(run(&desk, &f), SampleReport::Passed { .. }));
    let request = seen.lock().unwrap().pop().unwrap();
    assert!(segment::description(&request).is_some());
    assert!(segment::clicks(&request).unwrap().is_empty());
}
