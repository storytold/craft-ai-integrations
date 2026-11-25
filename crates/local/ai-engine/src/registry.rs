//! The one place engines are listed. An engine crate exposes a function that returns its runners;
//! an app (or the launcher) puts them in a registry and hands that to the client. Adding a model
//! to an engine, or an engine to an app, is a line here and nowhere else.

use std::sync::Arc;

use crate::runner::ModelRunner;

/// Every model runner this build can run, by engine and model id.
#[derive(Clone, Default)]
pub struct EngineRegistry {
    runners: Vec<Arc<dyn ModelRunner>>,
}

impl EngineRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a runner. One for the same engine and model id replaces the earlier one, so an app
    /// can swap in its own build of a model.
    pub fn register(&mut self, runner: impl ModelRunner + 'static) -> &mut Self {
        self.register_arc(Arc::new(runner))
    }

    pub fn register_arc(&mut self, runner: Arc<dyn ModelRunner>) -> &mut Self {
        let model = runner.model().id;
        let engine = runner.engine().to_ascii_lowercase();
        self.runners
            .retain(|r| !(r.engine().eq_ignore_ascii_case(&engine) && r.model().id == model));
        self.runners.push(runner);
        self
    }

    /// [`register`](Self::register) for chaining at construction.
    pub fn with(mut self, runner: impl ModelRunner + 'static) -> Self {
        self.register(runner);
        self
    }

    /// Adds every runner of `other`, as an engine crate's `runners()` hands them over.
    pub fn extend(&mut self, other: impl IntoIterator<Item = Arc<dyn ModelRunner>>) -> &mut Self {
        for runner in other {
            self.register_arc(runner);
        }
        self
    }

    /// The runner for a model on an engine. Engine names are matched ignoring case.
    pub fn find(&self, engine: &str, model: &str) -> Option<Arc<dyn ModelRunner>> {
        self.runners
            .iter()
            .find(|r| r.engine().eq_ignore_ascii_case(engine) && r.model().id.as_str() == model)
            .cloned()
    }

    /// The engines that have at least one runner, once each, lower case, in registration order.
    pub fn engines(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for r in &self.runners {
            let name = r.engine().to_ascii_lowercase();
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }

    /// The runners of one engine.
    pub fn runners_of(&self, engine: &str) -> Vec<Arc<dyn ModelRunner>> {
        self.runners
            .iter()
            .filter(|r| r.engine().eq_ignore_ascii_case(engine))
            .cloned()
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.runners.is_empty()
    }
}

impl std::fmt::Debug for EngineRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut rows: Vec<String> = self
            .runners
            .iter()
            .map(|r| format!("{}/{}", r.engine(), r.model().id))
            .collect();
        rows.sort();
        write!(f, "EngineRegistry({rows:?})")
    }
}
