//! Moving to the processor when the graphics card cannot run the network.
//!
//! A card can answer the engine's start-up probe and still fail on the network: on a build server
//! a software renderer (lavapipe) did, while reading the answer back. So the first failure that is
//! not a cancel builds the network again on the processor and runs that picture again there. Once
//! that rebuild has worked the model stays on the processor. The move is tried once: if the rebuild
//! fails, its error is the answer and later pictures go to the card again.

use craft_ai_core::{AiError, ErrorKind};
use craft_ai_engine::RunContext;

use super::Predict;
use super::pipeline::Input;

/// Loads the network with `card` when `on_card`, else with `processor` alone. A card load that
/// fails for any reason but the file is retried with `processor`; a file that is not the checkpoint
/// ([`ErrorKind::Unsupported`]) is refused at once instead of being read a second time. Returns the
/// network and whether it is on the card.
pub(super) fn load_or_processor<T>(
    on_card: bool,
    card: impl FnOnce() -> Result<T, AiError>,
    processor: impl FnOnce() -> Result<T, AiError>,
) -> Result<(T, bool), AiError> {
    if !on_card {
        return Ok((processor()?, false));
    }
    match card() {
        Ok(network) => Ok((network, true)),
        Err(error) if error.kind == ErrorKind::Unsupported => Err(error),
        Err(_) => Ok((processor()?, false)),
    }
}

/// Builds the network again on the processor. Called at most once.
pub(super) type Rebuild = Box<dyn FnOnce() -> Result<Box<dyn Predict>, AiError> + Send>;

/// The network on a graphics card, and the way to build it on the processor instead.
pub(super) struct WithFallback {
    current: Box<dyn Predict>,
    /// `None` once the network has moved to the processor.
    rebuild: Option<Rebuild>,
}

impl WithFallback {
    pub(super) fn new(on_card: Box<dyn Predict>, rebuild: Rebuild) -> Self {
        Self {
            current: on_card,
            rebuild: Some(rebuild),
        }
    }
}

impl Predict for WithFallback {
    fn predict(&mut self, input: &Input, ctx: &RunContext) -> Result<Vec<f32>, AiError> {
        let failed = match self.current.predict(input, ctx) {
            Err(error) if error.kind != ErrorKind::Cancelled => error,
            answer => return answer,
        };
        let Some(rebuild) = self.rebuild.take() else {
            return Err(failed);
        };
        ctx.notice(
            "The graphics card could not run the background remover, so it continues on the processor. This is slower.",
        );
        self.current = rebuild()?;
        self.current.predict(input, ctx)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Answers every pixel with its number, counting its calls.
    struct Fixed(f32, Arc<AtomicUsize>);

    impl Predict for Fixed {
        fn predict(&mut self, input: &Input, _: &RunContext) -> Result<Vec<f32>, AiError> {
            self.1.fetch_add(1, Ordering::SeqCst);
            Ok(vec![self.0; input.width * input.height])
        }
    }

    /// Fails every call with an error of `kind`, counting its calls.
    struct Failing(ErrorKind, Arc<AtomicUsize>);

    impl Predict for Failing {
        fn predict(&mut self, _: &Input, _: &RunContext) -> Result<Vec<f32>, AiError> {
            self.1.fetch_add(1, Ordering::SeqCst);
            Err(AiError::new(self.0, "the card failed"))
        }
    }

    fn input() -> Input {
        Input {
            width: 4,
            height: 2,
            px: vec![[0.5; 3]; 8],
        }
    }

    /// A model on a card that fails with `kind`, whose rebuild answers 1.0. Returns the model and
    /// the counts of card calls, rebuilds and processor calls.
    fn failing_card(kind: ErrorKind) -> (WithFallback, [Arc<AtomicUsize>; 3]) {
        let counts = [(); 3].map(|_| Arc::new(AtomicUsize::new(0)));
        let [card, rebuilds, processor] = counts.clone();
        let model = WithFallback::new(
            Box::new(Failing(kind, card)),
            Box::new(move || {
                rebuilds.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(Fixed(1.0, processor)) as Box<dyn Predict>)
            }),
        );
        (model, counts)
    }

    fn count(c: &Arc<AtomicUsize>) -> usize {
        c.load(Ordering::SeqCst)
    }

    #[test]
    fn a_card_that_works_is_kept() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut model = WithFallback::new(
            Box::new(Fixed(0.25, calls.clone())),
            Box::new(|| Err(AiError::invalid("no rebuild expected"))),
        );
        for _ in 0..2 {
            let answer = model.predict(&input(), &RunContext::detached()).unwrap();
            assert_eq!(answer, vec![0.25; 8]);
        }
        assert_eq!(count(&calls), 2);
    }

    #[test]
    fn a_failing_card_moves_to_the_processor_once_and_stays_there() {
        let (mut model, [card, rebuilds, processor]) = failing_card(ErrorKind::Internal);
        let ctx = RunContext::detached();
        assert_eq!(model.predict(&input(), &ctx).unwrap(), vec![1.0; 8]);
        assert_eq!(model.predict(&input(), &ctx).unwrap(), vec![1.0; 8]);
        assert_eq!(
            (count(&card), count(&rebuilds), count(&processor)),
            (1, 1, 2)
        );
    }

    #[test]
    fn a_cancel_is_not_a_failure() {
        let (mut model, [card, rebuilds, _]) = failing_card(ErrorKind::Cancelled);
        let error = model
            .predict(&input(), &RunContext::detached())
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Cancelled);
        assert_eq!((count(&card), count(&rebuilds)), (1, 0));
    }

    /// Loads that answer `Ok(name)` or fail with `kind`, recording which ran.
    fn load(
        name: &'static str,
        fails: Option<ErrorKind>,
        ran: &std::sync::Mutex<Vec<&'static str>>,
    ) -> Result<&'static str, AiError> {
        ran.lock().unwrap().push(name);
        match fails {
            Some(kind) => Err(AiError::new(kind, "failed")),
            None => Ok(name),
        }
    }

    #[test]
    fn a_card_that_loads_keeps_the_network() {
        let ran = std::sync::Mutex::new(Vec::new());
        let got = load_or_processor(
            true,
            || load("card", None, &ran),
            || load("cpu", None, &ran),
        );
        assert_eq!(got.unwrap(), ("card", true));
        assert_eq!(*ran.lock().unwrap(), ["card"]);
    }

    #[test]
    fn a_card_that_fails_to_load_loads_on_the_processor() {
        let ran = std::sync::Mutex::new(Vec::new());
        let got = load_or_processor(
            true,
            || load("card", Some(ErrorKind::Provider), &ran),
            || load("cpu", None, &ran),
        );
        assert_eq!(got.unwrap(), ("cpu", false));
        assert_eq!(*ran.lock().unwrap(), ["card", "cpu"]);
    }

    #[test]
    fn a_file_that_is_not_the_checkpoint_is_not_read_twice() {
        let ran = std::sync::Mutex::new(Vec::new());
        let got = load_or_processor(
            true,
            || load("card", Some(ErrorKind::Unsupported), &ran),
            || load("cpu", None, &ran),
        );
        assert_eq!(got.unwrap_err().kind, ErrorKind::Unsupported);
        assert_eq!(*ran.lock().unwrap(), ["card"]);
    }

    #[test]
    fn on_the_processor_there_is_one_load() {
        let ran = std::sync::Mutex::new(Vec::new());
        let got = load_or_processor(
            false,
            || load("card", None, &ran),
            || load("cpu", Some(ErrorKind::Provider), &ran),
        );
        assert_eq!(got.unwrap_err().kind, ErrorKind::Provider);
        assert_eq!(*ran.lock().unwrap(), ["cpu"]);
    }

    #[test]
    fn a_rebuild_that_fails_is_the_answer() {
        let card = Arc::new(AtomicUsize::new(0));
        let mut model = WithFallback::new(
            Box::new(Failing(ErrorKind::Provider, card)),
            Box::new(|| Err(AiError::unsupported("the checkpoint went away"))),
        );
        let error = model
            .predict(&input(), &RunContext::detached())
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Unsupported);
        // Nothing is left to try: the card's own failure from now on.
        let again = model
            .predict(&input(), &RunContext::detached())
            .unwrap_err();
        assert_eq!(again.kind, ErrorKind::Provider);
    }
}
