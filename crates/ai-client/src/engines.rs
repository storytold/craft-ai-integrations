//! Turning a registry of built-in model runners into adapters the client can build, and keeping
//! the providers built for each connection so their loaded models can be listed and freed.
//!
//! Invariants: one provider per connection per engine, replaced only when the connection names
//! different models or weight files. A replaced provider is asked to unload at once (a running
//! job finishes first) and stays listed until its models are gone, so nothing it holds is hidden.
//! Listing and unloading never block on a running job.
//!
//! The cache belongs to the factory and is shared by its clones, so clients built from one
//! factory share one loaded copy of each model rather than loading the weights twice.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use craft_ai_core::{AiError, Connection, ConnectionId, ModelId, Provider};
use craft_ai_engine::{EngineProvider, EngineRegistry, FreeMemory, LoadedModelInfo, Unloaded};

use crate::factory::ProviderFactory;

/// A model a built-in engine holds in memory, with the connection it was set up through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedEngineModel {
    /// The engine running it (`candle`, `rten`, `burn`).
    pub engine: String,
    pub connection: ConnectionId,
    /// The connection's name as a person sees it.
    pub connection_label: String,
    pub model: LoadedModelInfo,
}

impl ProviderFactory {
    /// Makes every engine in `registry` available: a saved connection whose provider is one of
    /// the registry's engines (`candle`, `burn`, `rten`) is built as an [`EngineProvider`] that runs its
    /// model from the weights file recorded on the connection. The accelerators each engine
    /// declares are recorded too, so engine advice needs nothing stated by the app.
    ///
    /// This is the whole of enabling a built-in engine in an app.
    pub fn register_engines(&mut self, registry: EngineRegistry) {
        let registry = Arc::new(registry);
        for engine in registry.engines() {
            let mut vendors = Vec::new();
            for runner in registry.runners_of(&engine) {
                for vendor in runner.accelerators() {
                    if !vendors.contains(vendor) {
                        vendors.push(*vendor);
                    }
                }
            }
            let registry = registry.clone();
            let kept = Arc::new(Kept::default());
            kept.set_free_memory(self.free_memory.clone());
            let cache = kept.clone();
            self.register(&engine, move |connection, _| {
                kept.provider_for(&registry, connection)
            });
            // After registering: registering a name forgets any earlier declaration and cache.
            self.declare_accelerators(&engine, &vendors);
            self.engines.insert(engine.to_ascii_lowercase(), cache);
        }
    }

    /// Checks free memory before a built-in engine loads a model, for engines registered before
    /// or after this call and for providers already built.
    pub fn set_free_memory(&mut self, read: Option<FreeMemory>) {
        for kept in self.engines.values() {
            kept.set_free_memory(read.clone());
        }
        self.free_memory = read;
    }

    /// Every model the built-in engines hold in memory now.
    pub(crate) fn loaded_models(&self) -> Vec<LoadedEngineModel> {
        let mut engines: Vec<_> = self.engines.iter().collect();
        engines.sort_by(|a, b| a.0.cmp(b.0));
        engines
            .into_iter()
            .flat_map(|(engine, kept)| kept.loaded(engine))
            .collect()
    }

    /// Unloads the models that `pick` selects (by connection and model), or with `idle`, those
    /// unused for at least that long.
    pub(crate) fn unload_models(
        &self,
        pick: Option<(&ConnectionId, &ModelId)>,
        idle: Option<Duration>,
    ) -> Unloaded {
        let mut done = Unloaded::default();
        for kept in self.engines.values() {
            let part = kept.unload(pick, idle);
            done.freed.extend(part.freed);
            done.after_run.extend(part.after_run);
        }
        done
    }
}

/// One built provider and what it was built from.
struct Built {
    /// Change any of it and the old provider no longer applies.
    shape: String,
    label: String,
    provider: Arc<EngineProvider>,
}

/// The provider last built for each connection of one engine.
///
/// The client asks the factory for a provider on every job. A model is slow to load and a runner
/// may keep work between jobs (SAM 3 keeps the picture it has looked at), so handing back a new
/// provider each time would load the weights again for every click, and two jobs at once would
/// load two copies. One provider per connection is kept instead, and replaced when the connection
/// names different models or weight files.
#[derive(Default)]
pub(crate) struct Kept {
    built: Mutex<HashMap<String, Built>>,
    /// Providers replaced while a job still held one of their models: listed and freeable until
    /// those models are gone.
    retiring: Mutex<Vec<(String, String, Arc<EngineProvider>)>>,
    /// Read by every provider at load time, so a reader set later still applies.
    free: Arc<Mutex<Option<FreeMemory>>>,
}

impl Kept {
    fn built(&self) -> MutexGuard<'_, HashMap<String, Built>> {
        self.built.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set_free_memory(&self, read: Option<FreeMemory>) {
        *self.free.lock().unwrap_or_else(PoisonError::into_inner) = read;
    }

    fn provider_for(
        &self,
        registry: &EngineRegistry,
        connection: &Connection,
    ) -> Result<Arc<dyn Provider>, AiError> {
        let mut shape = connection.provider.to_ascii_lowercase();
        for model in &connection.models {
            shape.push_str(&format!(
                "\n{}\t{}",
                model.id,
                model.local_file.as_deref().unwrap_or_default()
            ));
        }
        let mut built = self.built();
        let id = connection.id.to_string();
        if let Some(same) = built.get_mut(&id)
            && same.shape == shape
        {
            same.label.clone_from(&connection.label);
            return Ok(same.provider.clone());
        }
        let free = self.free.clone();
        // The reader is taken out of the lock before it runs, so a slow reader blocks nobody.
        let read: FreeMemory = Arc::new(move || {
            let current = free.lock().unwrap_or_else(PoisonError::into_inner).clone();
            current.and_then(|read| read())
        });
        let provider =
            Arc::new(EngineProvider::from_connection(registry, connection)?.with_free_memory(read));
        let replaced = built.insert(
            id.clone(),
            Built {
                shape,
                label: connection.label.clone(),
                provider: provider.clone(),
            },
        );
        if let Some(old) = replaced {
            // Jobs already submitted to the old provider still run. Retired, it lets go of every
            // model as its job finishes, and it stays listed until it holds nothing and has no
            // job left that could load one, so the memory is never out of sight. It moves to
            // the retiring list before the cache lock is released, so a listing never misses it.
            old.provider.retire();
            self.retiring
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((id, old.label, old.provider));
        }
        drop(built);
        Ok(provider)
    }

    /// The providers, current ones first, then retiring ones that still hold a model, copied
    /// out so no cache lock is held while they are asked anything.
    fn providers(&self) -> Vec<(String, String, Arc<EngineProvider>)> {
        let mut out: Vec<_> = self
            .built()
            .iter()
            .map(|(id, b)| (id.clone(), b.label.clone(), b.provider.clone()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        let mut retiring = self.retiring.lock().unwrap_or_else(PoisonError::into_inner);
        // Kept while anything else still holds it: a caller that got it before the swap may
        // submit through it later, and the model that job loads must stay in sight.
        retiring.retain(|(_, _, p)| {
            Arc::strong_count(p) > 1 || p.is_busy() || !p.loaded_models().is_empty()
        });
        out.extend(retiring.iter().cloned());
        out
    }

    fn loaded(&self, engine: &str) -> Vec<LoadedEngineModel> {
        let mut out = Vec::new();
        for (id, label, provider) in self.providers() {
            let Ok(connection) = ConnectionId::new(&id) else {
                continue;
            };
            for model in provider.loaded_models() {
                out.push(LoadedEngineModel {
                    engine: engine.to_owned(),
                    connection: connection.clone(),
                    connection_label: label.clone(),
                    model,
                });
            }
        }
        out
    }

    fn unload(&self, pick: Option<(&ConnectionId, &ModelId)>, idle: Option<Duration>) -> Unloaded {
        let mut done = Unloaded::default();
        for (id, _, provider) in self.providers() {
            let part = match (pick, idle) {
                (_, Some(older_than)) => provider.unload_idle(older_than),
                (Some((connection, model)), None) if connection.as_str() == id => {
                    provider.unload(Some(model))
                }
                (Some(_), None) => continue,
                (None, None) => provider.unload(None),
            };
            done.freed.extend(part.freed);
            done.after_run.extend(part.after_run);
        }
        done
    }
}

/// The engines compiled into this build, by cargo feature. Empty when none is turned on.
pub(crate) fn built_in() -> EngineRegistry {
    #[allow(unused_mut)]
    let mut registry = EngineRegistry::new();
    #[cfg(feature = "candle")]
    registry.extend(craft_ai_engine_candle::runners());
    #[cfg(feature = "rten")]
    registry.extend(craft_ai_engine_rten::runners());
    #[cfg(feature = "burn")]
    registry.extend(craft_ai_engine_burn::runners());
    registry
}
