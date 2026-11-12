//! The built-in ComfyUI adapter. A job is built from the same read of the workflow library that
//! its agreement was checked against: reading the library a second time to build the adapter
//! would let a workflow replaced in between run steps nobody agreed to.
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use craft_ai_comfyui::{ComfyConfig, ComfyProvider, Workflow, WorkflowLibrary};
use craft_ai_core::{AiError, Connection, ModelId, Provider};

use super::ProviderFactory;

/// The name the built-in ComfyUI adapter is registered under.
pub(super) const COMFYUI: &str = "comfyui";

/// Refusal when the workflow's steps are not the ones the plan named.
pub(crate) const WORKFLOW_CHANGED: &str =
    "the workflow changed after this job was planned; start it again";

impl ProviderFactory {
    /// The steps of the ComfyUI workflow `model` that are not known to keep data on this
    /// computer, read from the library the ComfyUI adapter reads. `None` when the library or the
    /// workflow cannot be read; building the adapter then fails the same way, so nothing is sent.
    pub(crate) fn comfyui_unknown_steps(&self, model: &ModelId) -> Option<Vec<String>> {
        load(self.comfyui_library.as_deref())
            .ok()?
            .iter()
            .find(|w| w.id() == model)
            .map(Workflow::unknown_steps)
    }

    /// Loads the built-in adapter's workflows before key access and checks the selected model's
    /// steps against `agreed`. The returned snapshot is the only graph source used to build the
    /// job. Missing or unreadable selected workflows fail here, before a key store is asked.
    /// `None` leaves an app's replacement ComfyUI builder in control as before.
    pub(crate) fn prepare_comfyui_agreed(
        &self,
        model: &ModelId,
        agreed: &[String],
    ) -> Option<Result<PreparedComfyui, AiError>> {
        if !self.comfyui_built_in {
            return None;
        }
        let prepare = || {
            let workflows = load(self.comfyui_library.as_deref())?;
            let steps = workflows
                .iter()
                .find(|w| w.id() == model)
                .map(Workflow::unknown_steps)
                .ok_or_else(|| {
                    AiError::invalid("the selected workflow is missing or cannot be read")
                })?;
            if steps != agreed {
                return Err(AiError::invalid(WORKFLOW_CHANGED));
            }
            Ok(PreparedComfyui { workflows })
        };
        Some(prepare())
    }

    /// Registers the built-in ComfyUI adapter with this factory's library, output folder and
    /// timeouts.
    pub(super) fn register_comfyui(&mut self) {
        let library = self.comfyui_library.clone();
        let output_dir = self.comfyui_output_dir.clone();
        let timeouts = self.timeouts;
        self.register(COMFYUI, move |c, _| {
            provider(
                c,
                load(library.as_deref())?,
                output_dir.as_deref(),
                timeouts,
            )
        });
        self.comfyui_built_in = true;
    }
}

/// The immutable workflows whose selected model passed the consent check before key access.
/// Kept across that lookup so another import cannot substitute a different graph for the job.
pub(crate) struct PreparedComfyui {
    workflows: Vec<Workflow>,
}

impl PreparedComfyui {
    /// Builds from the checked workflows without reading the library again, retaining the
    /// factory's output folder, timeouts and optional durable-media wrapper.
    pub(crate) fn build(
        self,
        factory: &ProviderFactory,
        connection: &Connection,
    ) -> Result<Arc<dyn Provider>, AiError> {
        let provider = provider(
            connection,
            self.workflows,
            factory.comfyui_output_dir.as_deref(),
            factory.timeouts,
        )?;
        factory.with_media(connection, provider)
    }
}

/// Every workflow in `dir`, or in the shared default library when `dir` is `None`.
fn load(dir: Option<&Path>) -> Result<Vec<Workflow>, AiError> {
    let library = match dir {
        Some(dir) => WorkflowLibrary::at(dir)?,
        None => WorkflowLibrary::at_default_location()
            .ok_or_else(|| AiError::invalid("could not locate the workflow library directory"))?,
    };
    library.load()
}

/// The ComfyUI adapter for `connection` serving exactly `workflows`.
fn provider(
    connection: &Connection,
    workflows: Vec<Workflow>,
    output_dir: Option<&Path>,
    timeouts: Option<(Duration, Duration)>,
) -> Result<Arc<dyn Provider>, AiError> {
    let base = connection
        .endpoint
        .as_deref()
        .ok_or_else(|| AiError::invalid("ComfyUI address is required"))?;
    if workflows.is_empty() {
        return Err(AiError::unsupported(
            "the workflow library is empty; import a workflow JSON first",
        ));
    }
    let mut cfg = ComfyConfig::new(base);
    if let Some(dir) = output_dir {
        cfg = cfg.with_output_dir(dir);
    }
    for w in workflows {
        cfg = cfg.with_workflow(w);
    }
    if let Some((connect, request)) = timeouts {
        cfg.connect_timeout = connect;
        cfg.request_timeout = request;
    }
    Ok(Arc::new(ComfyProvider::new(cfg)?))
}
