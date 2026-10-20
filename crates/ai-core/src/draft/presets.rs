//! Built-in connection presets and their explicitly unverified static model lists.
//! Keep these separate from draft validation so adding a service cannot change save rules.

use super::Preset;
use crate::connection::Billing;
use crate::model::{CapabilitySpec, InputRole, ModelInfo};
use crate::{CapabilityId, ModelId};

/// The services most people connect, in the order a form lists them.
pub const PRESETS: &[Preset] = &[
    Preset {
        key: "openai",
        name: "OpenAI",
        provider: "openai",
        endpoint: "https://api.openai.com/v1",
        billing: Billing::Metered,
        needs_key: true,
        note: "Your own OpenAI API key. Billed by OpenAI.",
    },
    Preset {
        key: "openrouter",
        name: "OpenRouter",
        provider: "openrouter",
        endpoint: "https://openrouter.ai/api/v1",
        billing: Billing::Metered,
        needs_key: true,
        note: "One key for many hosted models.",
    },
    Preset {
        key: "bfl",
        name: "Black Forest Labs",
        provider: "bfl",
        endpoint: "https://api.bfl.ai/v1",
        billing: Billing::Metered,
        needs_key: true,
        note: "FLUX 3 image and video generation. Billed by Black Forest Labs.",
    },
    Preset {
        key: "seedance",
        name: "Seedance (BytePlus ModelArk)",
        provider: "seedance",
        endpoint: "https://ark.ap-southeast.bytepluses.com/api/v3",
        billing: Billing::Metered,
        needs_key: true,
        note: "Seedance 2.0 text-to-video with your BytePlus ModelArk API key.",
    },
    Preset {
        key: "vertex-express",
        name: "Vertex AI (express mode)",
        provider: "vertex-express",
        endpoint: "https://aiplatform.googleapis.com/v1",
        billing: Billing::Metered,
        needs_key: true,
        note: "Gemini image generation. API key only; full Vertex uses different credentials. Key cannot be tested without a paid request.",
    },
    Preset {
        key: "artcraft-omni",
        name: "ArtCraft Omni API key",
        provider: "artcraft-omni",
        endpoint: "https://api.storyteller.ai",
        billing: Billing::Unknown,
        needs_key: true,
        note: "Staff-enabled API key for ArtCraft image and video generation. Subscription allowance is unverified; no free key test is documented.",
    },
    Preset {
        key: "ollama",
        name: "Ollama",
        provider: "ollama",
        endpoint: "http://127.0.0.1:11434/v1",
        billing: Billing::Free,
        needs_key: false,
        note: "Models running on this computer. Nothing leaves it.",
    },
    Preset {
        key: "lmstudio",
        name: "LM Studio",
        provider: "lmstudio",
        endpoint: "http://127.0.0.1:1234/v1",
        billing: Billing::Free,
        needs_key: false,
        note: "Models running on this computer. Nothing leaves it.",
    },
    Preset {
        key: "llamacpp",
        name: "llama.cpp",
        provider: "llamacpp",
        endpoint: "http://127.0.0.1:8080/v1",
        billing: Billing::Free,
        needs_key: false,
        note: "A llama.cpp server on this computer.",
    },
    Preset {
        key: "comfyui",
        name: "ComfyUI (local)",
        provider: "comfyui",
        endpoint: "http://127.0.0.1:8188",
        billing: Billing::Free,
        needs_key: false,
        note: "Local workflows on this computer. Import workflow JSON files first; each becomes a model.",
    },
    Preset {
        key: "custom",
        name: "Other (OpenAI compatible)",
        provider: "openai-compatible",
        endpoint: "",
        billing: Billing::Unknown,
        needs_key: true,
        note: "Any server that speaks the OpenAI API.",
    },
];

pub(super) fn preset_models(preset: &Preset) -> Vec<ModelInfo> {
    if preset.key == "artcraft-omni" {
        return [
            ("nano_banana", "Nano Banana (unverified)", "image.generate"),
            (
                "seedance_2p0",
                "Seedance 2.0 (unverified)",
                "video.generate",
            ),
        ]
        .into_iter()
        .filter_map(|(id, label, capability)| {
            Some(
                ModelInfo::new(ModelId::new(id).ok()?, label).with(
                    CapabilitySpec::new(CapabilityId::parse(capability).ok()?)
                        .accepting(InputRole::Prompt, 1)
                        .requiring(InputRole::Prompt),
                ),
            )
        })
        .collect();
    }
    // Vertex express does not offer a known free key probe. Seed the documented model as
    // unverified so it can be bound without pretending the Test button has reached Google.
    if preset.key != "vertex-express" {
        return Vec::new();
    }
    match (
        ModelId::new("gemini-2.5-flash-image"),
        CapabilityId::parse("image.generate"),
    ) {
        (Ok(id), Ok(cap)) => vec![
            ModelInfo::new(id, "Gemini 2.5 Flash Image (unverified)").with(
                CapabilitySpec::new(cap)
                    .accepting(InputRole::Prompt, 1)
                    .requiring(InputRole::Prompt),
            ),
        ],
        _ => Vec::new(),
    }
}
