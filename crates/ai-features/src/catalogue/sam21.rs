//! Meta's SAM 2.1, run through ComfyUI with Florence-2-large finding the words of a prompt.

use super::florence2::florence2_large_files;
use crate::recipe::{Component, Licence, LocalRecipe, ModelFile, NodePack, Source};

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
        native: Vec::new(),
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
