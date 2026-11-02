//! Executable Omni models, separate from account verification.
use craft_ai_core::{CapabilityId, CapabilitySpec, InputRole, ModelId, ModelInfo};

/// Documented Banana image editing uses two reference images and an instruction, not a mask API.
pub(crate) fn documented() -> Vec<ModelInfo> {
    let mut models: Vec<_> = [
        ("nano_banana", "Nano Banana (unverified)"),
        ("nano_banana_2", "Nano Banana 2 (unverified)"),
        ("nano_banana_pro", "Nano Banana Pro (unverified)"),
    ]
    .into_iter()
    .filter_map(|(id, label)| {
        Some(
            ModelInfo::new(ModelId::new(id).ok()?, label)
                .with(
                    CapabilitySpec::new(CapabilityId::parse("image.generate").ok()?)
                        .accepting(InputRole::Prompt, 1)
                        .requiring(InputRole::Prompt),
                )
                .with(
                    CapabilitySpec::new(CapabilityId::parse("image.inpaint").ok()?)
                        .accepting(InputRole::Source, 1)
                        .requiring(InputRole::Source)
                        .accepting(InputRole::Mask, 1)
                        .requiring(InputRole::Mask)
                        .accepting(InputRole::Prompt, 1)
                        .requiring(InputRole::Prompt),
                ),
        )
    })
    .collect();
    if let (Ok(id), Ok(cap)) = (
        ModelId::new("seedance_2p0"),
        CapabilityId::parse("video.generate"),
    ) {
        models.push(
            ModelInfo::new(id, "Seedance 2.0 (unverified)").with(
                CapabilitySpec::new(cap)
                    .accepting(InputRole::Prompt, 1)
                    .requiring(InputRole::Prompt),
            ),
        );
    }
    models
}
