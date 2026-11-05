//! A model-list-only provider shared by saved-probe integration fixtures.
use craft_ai_core::{AiError, JobId, JobSnapshot, ModelId, ModelInfo, Provider, Request};

pub struct ModelProbe<F> {
    read: F,
}

impl<F> ModelProbe<F> {
    /// Uses `read` to control when and how the model-list answer arrives.
    pub fn new(read: F) -> Self {
        Self { read }
    }
}

impl<F> Provider for ModelProbe<F>
where
    F: Fn() -> Result<Vec<ModelInfo>, AiError> + Send + Sync,
{
    /// Runs the fixture's model-list operation, including any intervening store edit.
    fn models(&self) -> Result<Vec<ModelInfo>, AiError> {
        (self.read)()
    }
    /// A model-list probe cannot submit a job.
    fn submit(&self, _: &ModelId, _: &Request) -> Result<JobId, AiError> {
        Err(AiError::unsupported("not a job"))
    }
    /// A model-list probe has no jobs to poll.
    fn poll(&self, _: &JobId) -> Result<JobSnapshot, AiError> {
        Err(AiError::unsupported("not a job"))
    }
    /// A model-list probe has no work to cancel.
    fn cancel(&self, _: &JobId) -> Result<(), AiError> {
        Err(AiError::unsupported("not a job"))
    }
}
