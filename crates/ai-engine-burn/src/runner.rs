//! SAM 2.1 on Burn as a [`ModelRunner`]: the whole of what an app gets from this engine.
//!
//! The request and answer are the shared `image.segment` contract, handled once for every SAM 2.1
//! engine by [`Loaded`](craft_ai_engine_rten::Loaded): a PNG or JPEG `source`, clicks or a box in
//! params, a grayscale PNG mask at the picture's size, then the `Found` result. This file only
//! says which engine and model this is and how to load its weights.
//!
//! The weights are not the published ONNX files. They are the saved copy that
//! `craft-ai-burn-setup` makes once on this computer. Given the downloaded `encoder.onnx`, this
//! runner prepares that copy itself (see `prepare`), then loads it.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;

use craft_ai_core::{AiError, CapabilityId, CapabilitySpec, InputRole, ModelId, ModelInfo};
use craft_ai_engine::{LoadedModel, ModelRunner};
use craft_ai_engine_rten::Loaded;
use craft_ai_features::GpuVendor;

use crate::backend::{Burn, DECODER_FILE, ENCODER_FILE};

/// The engine and model ids as the catalogue writes them (`NativeRoute`).
pub const ENGINE: &str = "burn";
pub const MODEL: &str = "sam21";

/// The card makers Burn's graphics layer reaches (Vulkan, DirectX 12 and Metal). The same list the
/// engine advice table holds for this engine; the app's own build is what makes it true.
const CARDS: [GpuVendor; 4] = [
    GpuVendor::Nvidia,
    GpuVendor::Amd,
    GpuVendor::Intel,
    GpuVendor::Apple,
];

/// Every runner this crate offers, for an `EngineRegistry`.
pub fn runners() -> Vec<Arc<dyn ModelRunner>> {
    vec![
        Arc::new(Sam21Runner),
        Arc::new(crate::inspyrenet::InspyrenetRunner::new()),
    ]
}

/// SAM 2.1 on the Burn engine. It uses a graphics card when one answers, else the processor.
pub struct Sam21Runner;

impl ModelRunner for Sam21Runner {
    /// The engine name the catalogue writes.
    fn engine(&self) -> &str {
        ENGINE
    }

    /// Describes the model: `image.segment` with a required picture and an accepted prompt, as the
    /// other SAM 2.1 engine does, so the same tool can be pointed at either.
    fn model(&self) -> ModelInfo {
        let mut info = ModelInfo::new(model_id(), "SAM 2.1, graphics card");
        if let Ok(capability) = CapabilityId::parse("image.segment") {
            info = info.with(
                CapabilitySpec::new(capability)
                    .accepting(InputRole::Source, 1)
                    .accepting(InputRole::Prompt, 1)
                    .requiring(InputRole::Source),
            );
        }
        info
    }

    /// The card makers this engine can use.
    fn accelerators(&self) -> &[GpuVendor] {
        &CARDS
    }

    /// Loads both saved files when `weights` is the encoder and the decoder is beside it, and says
    /// what to do otherwise.
    fn load(&self, weights: &Path) -> Result<Box<dyn LoadedModel>, AiError> {
        if weights.file_name() == Some(OsStr::new("encoder.onnx")) {
            let dir = prepare::prepared_folder(weights)?;
            return Ok(Box::new(Loaded::new(Burn::load(&dir)?)));
        }
        let dir = weights
            .parent()
            .filter(|_| weights.file_name() == Some(OsStr::new(ENCODER_FILE)));
        let Some(dir) = dir.filter(|d| d.join(DECODER_FILE).is_file() && weights.is_file()) else {
            return Err(AiError::unsupported(
                "SAM 2.1 needs its prepared files, encoder.bpk and decoder.bpk, together in one folder. Open Set up AI to prepare them.",
            ));
        };
        Ok(Box::new(Loaded::new(Burn::load(dir)?)))
    }
}

/// The model id, from a literal that always parses.
#[allow(clippy::expect_used)] // `sam21` is a valid id: a literal that cannot fail
fn model_id() -> ModelId {
    ModelId::new(MODEL).expect("sam21 is a valid model id")
}

mod prepare;

#[cfg(test)]
mod tests;
