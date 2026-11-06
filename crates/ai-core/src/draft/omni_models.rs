//! The ArtCraft Omni models the adapter can run, for the preset and for connections saved earlier.
//!
//! This is the one list: the adapter (`ArtCraftProvider::documented_models`) reads it through
//! [`crate::artcraft_omni_models`], because this crate cannot depend on the adapter's crate. The
//! preset, the adapter and saved connections therefore cannot disagree about what a model can do.
//!
//! Invariant: refreshing a saved connection only adds a capability a saved model lacks. It never
//! adds or removes a model, renames one, or changes a capability already saved, so a person's model
//! list and every binding that names one of its models stay as they were.

use crate::connection::Connection;
use crate::model::{CapabilitySpec, InputRole, ModelInfo};
use crate::{CapabilityId, ModelId};

/// The provider name the Omni preset saves.
const PROVIDER: &str = "artcraft-omni";

/// The Omni models the adapter runs. Generative fill takes the picture and mask; the prompt is
/// optional, because an empty prompt asks Omni to remove what the mask covers.
pub fn documented() -> Vec<ModelInfo> {
    let image = [
        ("nano_banana", "Nano Banana (unverified)"),
        ("nano_banana_2", "Nano Banana 2 (unverified)"),
        ("nano_banana_pro", "Nano Banana Pro (unverified)"),
    ]
    .into_iter()
    .filter_map(|(id, label)| {
        Some(
            ModelInfo::new(ModelId::new(id).ok()?, label)
                .with(prompt_only("image.generate")?)
                .with(
                    CapabilitySpec::new(CapabilityId::parse("image.inpaint").ok()?)
                        .accepting(InputRole::Source, 1)
                        .requiring(InputRole::Source)
                        .accepting(InputRole::Mask, 1)
                        .requiring(InputRole::Mask)
                        .accepting(InputRole::Prompt, 1),
                ),
        )
    });
    let video = ModelId::new("seedance_2p0").ok().and_then(|id| {
        Some(ModelInfo::new(id, "Seedance 2.0 (unverified)").with(prompt_only("video.generate")?))
    });
    image.chain(video).collect()
}

/// A capability that takes exactly one prompt and needs it.
fn prompt_only(capability: &str) -> Option<CapabilitySpec> {
    Some(
        CapabilitySpec::new(CapabilityId::parse(capability).ok()?)
            .accepting(InputRole::Prompt, 1)
            .requiring(InputRole::Prompt),
    )
}

/// Gives an Omni connection saved before a capability existed the capabilities its models now
/// have. Any other connection is left alone.
pub(crate) fn refresh(connection: &mut Connection) {
    if !connection.provider.eq_ignore_ascii_case(PROVIDER) {
        return;
    }
    let known = documented();
    for saved in &mut connection.models {
        let Some(current) = known.iter().find(|m| m.id == saved.id) else {
            continue;
        };
        for spec in &current.capabilities {
            if saved.spec_for(&spec.capability).is_none() {
                saved.capabilities.push(spec.clone());
            }
        }
    }
}
