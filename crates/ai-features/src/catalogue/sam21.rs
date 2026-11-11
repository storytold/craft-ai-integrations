//! Meta's SAM 2.1, run through ComfyUI with Florence-2-large finding the words of a prompt.

use super::florence2::florence2_large_files;
use crate::recipe::{Component, Licence, LocalRecipe, ModelFile, NativeRoute, NodePack, Source};

/// The published ONNX export of SAM 2.1 (tiny size) at a pinned revision. The built-in rten
/// engine reads these two files as they are: no conversion, nothing hosted by us. The Burn
/// engine converts the same two once on the person's computer, so it lists them too. Both sit in
/// one folder, `encoder.onnx` first (the weights file the connection records).
fn sam21_onnx_files() -> Vec<ModelFile> {
    const REVISION: &str = "12bdf0f712731bd0b52ccbff807683ce49b726b9";
    let file = |name: &str, size_bytes: u64, sha256: &str| {
        ModelFile {
        folder: "sam21_onnx".to_owned(),
        filename: name.to_owned(),
        size_bytes,
        sha256: sha256.to_owned(),
        source: Source::HuggingFace {
            repo: "cstr/sam2.1-hiera-tiny-ONNX".to_owned(),
            file: name.to_owned(),
            revision: Some(REVISION.to_owned()),
        },
        licence: Licence {
            name: "Apache-2.0, ONNX conversion".to_owned(),
            url: Some("https://huggingface.co/cstr/sam2.1-hiera-tiny-ONNX".to_owned()),
            accept_required: false,
            note: Some(
                "This file is a format conversion of Meta's SAM 2.1 weights, which Meta releases under Apache 2.0. The publisher of the conversion declares the same licence."
                    .to_owned(),
            ),
        },
    }
    };
    vec![
        file(
            "encoder.onnx",
            109_473_919,
            "4fda6e68561e27808cedffa8689749a622bba5f11ba66f4ca4e3a8a9f6360db5",
        ),
        file(
            "decoder.onnx",
            20_655_795,
            "0ae944e08e55814c19afa4d143ef07b1a3dbad2decd77c15ec3ab220b9f57247",
        ),
    ]
}

/// SAM 2.1 for Select subject: the large checkpoint in the layout the
/// `ComfyUI-segment-anything-2` node pack reads, at a pinned revision of a community conversion,
/// then the Florence-2-large detector files. The node pack loads it at half precision by default,
/// which reads the `-fp16` file.
pub(super) fn sam21() -> LocalRecipe {
    const FILE: &str = "sam2.1_hiera_large-fp16.safetensors";
    let mut models = vec![ModelFile {
        folder: "sam2".to_owned(),
        filename: FILE.to_owned(),
        size_bytes: 448_999_828,
        sha256: "278af5296fcc153aa94645a4981e93fe8b7a936a9fa27dbd71e8c850df6d7038".to_owned(),
        source: Source::HuggingFace {
            repo: "Kijai/sam2-safetensors".to_owned(),
            file: FILE.to_owned(),
            revision: Some("f885607d88bb3f9145efa49c3e3c50a9e5bf13eb".to_owned()),
        },
        licence: Licence {
            name: "Apache-2.0, community conversion".to_owned(),
            url: Some("https://github.com/facebookresearch/sam2".to_owned()),
            accept_required: false,
            note: Some(
                "This file is a community conversion of Meta's SAM 2 weights, which Meta releases under Apache 2.0."
                    .to_owned(),
            ),
        },
    }];
    models.extend(florence2_large_files());
    LocalRecipe {
        label: "SAM 2.1".to_owned(),
        usable: true,
        workflow: "sam21-segment".to_owned(),
        comfyui: true,
        models,
        packs: vec![
            NodePack {
                label: "ComfyUI segment anything 2".to_owned(),
                repo_url: "https://github.com/kijai/ComfyUI-segment-anything-2".to_owned(),
                folder_names: vec!["comfyui-segment-anything-2".to_owned()],
            },
            NodePack {
                label: "ComfyUI Florence 2".to_owned(),
                repo_url: "https://github.com/kijai/ComfyUI-Florence2".to_owned(),
                folder_names: vec!["comfyui-florence2".to_owned()],
            },
        ],
        min_vram_mb: None,
        // Built-in engines that run SAM 2.1. It selects by clicking; a description needs the
        // ComfyUI route above (Florence-2 finds the words).
        native: vec![
            NativeRoute {
                provider: "rten".to_owned(),
                model: "sam21".to_owned(),
                label: "SAM 2.1, built in (the processor, on any system)".to_owned(),
                models: sam21_onnx_files(),
                note: Some("Selects by clicking on the picture, not by a description.".to_owned()),
                // About 35 ms a click after a first look of about 0.7 s, one Windows machine, one
                // picture (docs/STATUS.md). Not a benchmark.
                fast_on_processor: true,
            },
            // The same published files, converted once on this computer to the saved copy this
            // engine reads (`craft-ai-burn-setup`). Nothing converted is hosted, so the download is
            // the same two files and shares the folder with the route above. rten stays the first
            // and default route; this one is a choice.
            NativeRoute {
                provider: "burn".to_owned(),
                model: "sam21".to_owned(),
                label: "SAM 2.1, built in (any graphics card, set up once on this computer)"
                    .to_owned(),
                models: sam21_onnx_files(),
                note: Some("Selects by clicking on the picture, not by a description.".to_owned()),
                fast_on_processor: false,
            },
        ],
        components: components(),
    }
}

/// What the SAM 2.1 text-grounded path runs besides its checkpoint, with each publisher's own
/// statement as read on 20/10/2026. Disclosure only: see [`Component`].
fn components() -> Vec<Component> {
    let declared = |name: &str, url: &str, note: Option<&str>| Licence {
        name: name.to_owned(),
        url: Some(url.to_owned()),
        accept_required: false,
        note: note.map(str::to_owned),
    };
    vec![
        Component {
            label: "SAM 2.1 checkpoint".to_owned(),
            role: "Cuts the subject out".to_owned(),
            licence: declared(
                "Apache-2.0, as declared by Meta and by the conversion's publisher",
                "https://github.com/facebookresearch/sam2/blob/main/LICENSE",
                None,
            ),
        },
        Component {
            label: "ComfyUI segment anything 2 add-on".to_owned(),
            role: "Runs SAM 2.1 inside ComfyUI".to_owned(),
            licence: declared(
                "Apache-2.0, as declared by its publisher",
                "https://github.com/kijai/ComfyUI-segment-anything-2/blob/main/LICENSE",
                Some(
                    "If the checkpoint file is missing, this add-on downloads it from Hugging Face by itself. Craft's readiness scan compares file names and sizes; it does not by itself check the SHA-256 of an installed file.",
                ),
            ),
        },
        Component {
            label: "ComfyUI Florence 2 add-on".to_owned(),
            role: "Runs the detector inside ComfyUI".to_owned(),
            licence: declared(
                "MIT, as declared by its publisher",
                "https://github.com/kijai/ComfyUI-Florence2/blob/main/LICENSE",
                None,
            ),
        },
        Component {
            label: "Florence-2-large detector".to_owned(),
            role: "Finds the words you type in the picture".to_owned(),
            licence: declared(
                "MIT, as declared by Microsoft",
                "https://huggingface.co/microsoft/Florence-2-large/blob/main/LICENSE",
                Some(
                    "This one is set up for you. You can run another Florence-2 variant in ComfyUI yourself; its terms are its publisher's, and this list does not cover it.",
                ),
            ),
        },
    ]
}
