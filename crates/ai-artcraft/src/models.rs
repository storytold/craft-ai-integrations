//! Executable Omni models, separate from account verification.
use craft_ai_core::ModelInfo;

/// Banana image editing uses two reference images and an instruction, not a mask API. The list
/// lives in `craft-ai-core`, so the connection preset and saved connections offer exactly what
/// this adapter runs.
pub(crate) fn documented() -> Vec<ModelInfo> {
    craft_ai_core::artcraft_omni_models()
}
