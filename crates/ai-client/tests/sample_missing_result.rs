//! A selection sample passes only when the route returns the selection result beside its mask, as
//! the `image.segment` contract asks of every route. Scripted provider; no model or service.
use std::sync::Arc;
use std::time::Duration;

use craft_ai_client::{AiClient, ProviderFactory, SampleReport, TestStart, ToolDesk};
use craft_ai_core::job::{Event, Output};
use craft_ai_core::segment;
use craft_ai_core::{
    AiError, JobId, JobSnapshot, JobState, ModelId, ModelInfo, Payload, Provider, Request,
};
use craft_ai_features::builtin;
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;

/// Answers every job at once with a mask and nothing else.
struct MaskOnly;

impl Provider for MaskOnly {
    /// No model list; the connection carries it.
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        Ok(Vec::new())
    }
    /// Accepts any request.
    fn submit(&self, _: &ModelId, _: &Request) -> Result<JobId, AiError> {
        JobId::new("job-1").map_err(|e| AiError::invalid(e.to_string()))
    }
    /// A finished job holding one mask and no selection result.
    fn poll(&self, _: &JobId) -> Result<JobSnapshot, AiError> {
        let mut snap = JobSnapshot::new(JobState::Succeeded);
        snap.events.push(Event::Output(Output {
            payload: Payload::inline(segment::MASK_MEDIA_TYPE, vec![1, 2, 3])?,
            label: None,
        }));
        Ok(snap)
    }
    /// Nothing to stop: the job is already done.
    fn cancel(&self, _: &JobId) -> Result<(), AiError> {
        Ok(())
    }
}

/// A mask with no selection result beside it fails the test and says what is missing.
#[test]
fn a_mask_with_no_result_beside_it_fails_the_selection_test() {
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let mut factory = ProviderFactory::standard();
    factory.register("rten", |_, _| Ok(Arc::new(MaskOnly)));
    let client = AiClient::new(
        ToolDesk::app().unwrap(),
        shared.clone(),
        Arc::new(MemoryStore::new()),
    )
    .with_factory(factory);
    let desk = ToolDesk::new(client, shared, None);
    let weights = dir.path().join("encoder.onnx");
    std::fs::write(&weights, b"x").unwrap();
    let feature = builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    desk.enable_native_with(&feature, "rten", &weights).unwrap();
    let TestStart::Started(job) = desk.start_sample(&feature, None).unwrap() else {
        panic!("a local sample needs no agreement");
    };
    match ToolDesk::finish(&feature.role().unwrap(), &job, Duration::from_secs(5)) {
        SampleReport::Failed { message } => assert!(message.contains("did not say whether")),
        other => panic!("expected a failed test, got {other:?}"),
    }
}
