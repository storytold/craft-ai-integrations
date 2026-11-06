//! Meta's SAM 3, run through ComfyUI, with its checked weights file.

use crate::recipe::{Licence, LocalRecipe, ModelFile, NativeRoute, NodePack, Source};

/// Meta's SAM 3 checkpoint in the Hugging Face layout, at a pinned revision, with the tokenizer
/// files the text prompt needs. All three sit in one folder, which is how the loader finds them.
fn sam3_transformers_files() -> Vec<ModelFile> {
    const REVISION: &str = "3c879f39826c281e95690f02c7821c4de09afae7";
    let file = |name: &str, size_bytes: u64, sha256: &str| {
        ModelFile {
        folder: "sam3_transformers".to_owned(),
        filename: name.to_owned(),
        size_bytes,
        sha256: sha256.to_owned(),
        source: Source::HuggingFace {
            repo: "facebook/sam3".to_owned(),
            file: name.to_owned(),
            revision: Some(REVISION.to_owned()),
        },
        licence: Licence {
            name: "Meta SAM License".to_owned(),
            url: Some("https://huggingface.co/facebook/sam3".to_owned()),
            accept_required: true,
            note: Some(
                "Meta's release is gated: downloading needs a Hugging Face account that has been granted access."
                    .to_owned(),
            ),
        },
    }
    };
    vec![
        file(
            "model.safetensors",
            3_439_938_512,
            "6d06f0a5f84e435071fe6603e61d0b4cc7b40e0d39d487cfd4d67d8cc11cc14a",
        ),
        file(
            "vocab.json",
            862_328,
            "5047b556ce86ccaf6aa22b3ffccfc52d391ea4accdab9c2f2407da5b742d4363",
        ),
        file(
            "merges.txt",
            524_619,
            "9fd691f7c8039210e0fced15865466c65820d09b63988b0174bfe25de299051a",
        ),
    ]
}

/// SAM 3 for Select subject: the checkpoint, the node pack and the built-in runtimes that read it.
pub(super) fn sam3() -> LocalRecipe {
    LocalRecipe {
        label: "SAM 3".to_owned(),
        // PhotoCraft runs it today (Object Selection, `Sam3Session` through ComfyUI).
        usable: true,
        workflow: "sam3-segment".to_owned(),
        comfyui: true,
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
        // The built-in runtimes are ports of Hugging Face's implementation and read that
        // checkpoint layout, which is a different file from the ComfyUI one above (different
        // tensor names, plus the tokenizer files). Burn is a listed slot with no code yet, so
        // apps report it as unavailable.
        native: vec![
            NativeRoute {
                provider: "candle".to_owned(),
                model: "sam3".to_owned(),
                label: "SAM 3, built in (Apple graphics, otherwise the processor)".to_owned(),
                models: sam3_transformers_files(),
                note: None,
                fast_on_processor: false,
            },
            NativeRoute {
                provider: "burn".to_owned(),
                model: "sam3".to_owned(),
                // No Burn code exists yet, so no files are named for it: a route that cannot run must not
                // offer a 3.4 GB download.
                label: "SAM 3, built in (any graphics card)".to_owned(),
                models: Vec::new(),
                note: None,
                fast_on_processor: false,
            },
        ],
        components: Vec::new(),
    }
}
