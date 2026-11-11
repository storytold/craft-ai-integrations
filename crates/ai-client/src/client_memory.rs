//! Models the built-in engines keep in memory, as an app sees them: a list for Set up AI, and
//! ways to free them. The model files on disk are never touched, and a model in use finishes its
//! job before it is let go of.

use std::time::Duration;

use craft_ai_core::{ConnectionId, ModelId};
use craft_ai_engine::{FreeMemory, Unloaded};

use crate::client::AiClient;
use crate::engines::LoadedEngineModel;

impl AiClient {
    /// Every model the built-in engines hold in memory now, with roughly how much each holds.
    /// Only models this client's jobs loaded are listed; another app's are its own.
    pub fn loaded_models(&self) -> Vec<LoadedEngineModel> {
        self.factory.loaded_models()
    }

    /// Frees one model, set up through `connection`. The next job that needs it loads it again.
    pub fn unload_model(&self, connection: &ConnectionId, model: &ModelId) -> Unloaded {
        self.factory.unload_models(Some((connection, model)), None)
    }

    /// Frees every model the built-in engines hold.
    pub fn unload_all_models(&self) -> Unloaded {
        self.factory.unload_models(None, None)
    }

    /// Frees the models no job has used for at least `older_than`, leaving models in use alone.
    /// Nothing calls this on a timer; the app decides when.
    pub fn unload_idle_models(&self, older_than: Duration) -> Unloaded {
        self.factory.unload_models(None, Some(older_than))
    }

    /// Checks free memory before a built-in engine loads a model, so a load that will not fit
    /// fails in plain words instead of starting. `read` returns `None` when it cannot tell, and
    /// then nothing is refused.
    pub fn with_free_memory(mut self, read: FreeMemory) -> Self {
        self.factory.set_free_memory(Some(read));
        self
    }
}
