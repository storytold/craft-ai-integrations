//! Meta's SAM 3, run through ComfyUI, with its checked weights file.

use crate::recipe::{Licence, LocalRecipe, ModelFile, NativeRoute, NodePack, Source};

/// SAM 3 for Select subject: the checkpoint, the node pack and the built-in runtimes that read it.
pub(super) fn sam3() -> LocalRecipe {
    LocalRecipe {
        label: "SAM 3".to_owned(),
        // PhotoCraft runs it today (Object Selection, `Sam3Session` through ComfyUI).
        usable: true,
        workflow: "sam3-segment".to_owned(),
        models: vec![ModelFile {
            folder: "sam3".to_owned(),
            filename: "sam3.safetensors".to_owned(),
            size_bytes: 3_449_860_632,
            sha256: "127037a7a11169c63a210b8e3e9caad24a66abdd65d3a78bbc3a7d8577d57026".to_owned(),
            source: Source::HuggingFace {
                repo: "apozz/sam3-safetensors".to_owned(),
                file: "sam3.safetensors".to_owned(),
                revision: Some("e88da2ed02aded4006f4711ee6a9c75d5aa38d0c".to_owned()),
            },
            licence: Licence {
                name: "Meta SAM License, community copy".to_owned(),
                url: Some("https://huggingface.co/facebook/sam3".to_owned()),
                accept_required: true,
                note: Some(
                    "This file is a community copy of Meta's SAM 3 weights and declares no licence of its own. Meta's release is gated behind the SAM License."
                        .to_owned(),
                ),
            },
        }],
        packs: vec![NodePack {
            label: "ComfyUI-SAM3".to_owned(),
            repo_url: "https://github.com/PozzettiAndrea/ComfyUI-SAM3".to_owned(),
            folder_names: vec!["comfyui-sam3".to_owned()],
        }],
        min_vram_mb: None,
        // LightCraft's Candle runtime reads this same file (it understands Meta's layout). Burn is
        // a listed slot: no app has a Burn adapter yet, so apps report it as unavailable.
        native: vec![
            NativeRoute {
                provider: "candle".to_owned(),
                model: "sam3".to_owned(),
                label: "SAM 3, built in (Nvidia or Apple graphics)".to_owned(),
            },
            NativeRoute {
                provider: "burn".to_owned(),
                model: "sam3".to_owned(),
                label: "SAM 3, built in (any graphics card)".to_owned(),
            },
        ],
        components: Vec::new(),
    }
}
