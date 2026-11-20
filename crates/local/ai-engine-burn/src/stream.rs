//! One GPU stream for each loaded model.
//!
//! CubeCL, under Burn, gives every new thread its own stream by default and never hands a stream
//! number out twice. The provider runs each job on a new thread, and InSPyReNet starts another
//! one with a large stack, so every run landed on a stream the runs before it had not used, and
//! the memory they left in the pool was never reused: it grew with each run until the graphics
//! card was full, and runs then got many times slower.
//!
//! A model here gets a stream of its own when it loads and keeps it for every run, whichever
//! thread the run comes on. The process-wide stream policy is left alone, so other CubeCL users in
//! the same app are not affected.

use burn::tensor::StreamId;
use craft_ai_core::{AiError, Output, Request};
use craft_ai_engine::{LoadedModel, RunContext};

/// Runs `load` on a new stream and returns its model, which then runs on that stream too.
pub(crate) fn load_on_own_stream(
    load: impl FnOnce() -> Result<Box<dyn LoadedModel>, AiError>,
) -> Result<Box<dyn LoadedModel>, AiError> {
    let stream = StreamId::allocate();
    let model = stream.executes(load)?;
    Ok(Box::new(OnItsStream { stream, model }))
}

/// A loaded model and the stream it was loaded on.
struct OnItsStream {
    stream: StreamId,
    model: Box<dyn LoadedModel>,
}

impl LoadedModel for OnItsStream {
    /// Runs the model on its own stream.
    fn run(&mut self, request: &Request, ctx: &RunContext) -> Result<Vec<Output>, AiError> {
        let model = &mut self.model;
        self.stream.executes(|| model.run(request, ctx))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use craft_ai_core::{CapabilityId, Request};
    use craft_ai_engine::RunContext;

    use super::*;

    /// Writes down the stream every run is on.
    struct Recorder(Arc<Mutex<Vec<StreamId>>>);

    impl LoadedModel for Recorder {
        fn run(&mut self, _: &Request, _: &RunContext) -> Result<Vec<Output>, AiError> {
            self.0.lock().unwrap().push(StreamId::current());
            Ok(Vec::new())
        }
    }

    fn request() -> Request {
        Request::new(CapabilityId::parse("image.matte").unwrap())
    }

    #[test]
    fn every_run_uses_the_stream_the_model_loaded_on() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut loaded_on = None;
        let model = load_on_own_stream(|| {
            loaded_on = Some(StreamId::current());
            Ok(Box::new(Recorder(Arc::clone(&seen))))
        })
        .unwrap();
        let model = Arc::new(Mutex::new(model));
        // Each run on a new thread, as the provider does it.
        for _ in 0..3 {
            let model = Arc::clone(&model);
            std::thread::spawn(move || {
                model
                    .lock()
                    .unwrap()
                    .run(&request(), &RunContext::detached())
                    .unwrap();
            })
            .join()
            .unwrap();
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert!(
            seen.iter().all(|s| Some(*s) == loaded_on),
            "{loaded_on:?} vs {seen:?}"
        );
        assert_ne!(
            loaded_on,
            Some(StreamId::current()),
            "not the test thread's own stream"
        );
    }

    #[test]
    fn two_models_get_two_streams() {
        let mut first = None;
        let mut second = None;
        load_on_own_stream(|| {
            first = Some(StreamId::current());
            Ok(Box::new(Recorder(Arc::default())))
        })
        .unwrap();
        load_on_own_stream(|| {
            second = Some(StreamId::current());
            Ok(Box::new(Recorder(Arc::default())))
        })
        .unwrap();
        assert_ne!(first, second);
    }
}
