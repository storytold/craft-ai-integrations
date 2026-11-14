//! Depth Anything V2 Small for Estimate depth, run by the built-in Candle engine only. The model
//! code is in `craft-ai-engine-candle`'s `depth` module.

use crate::recipe::{Licence, LocalRecipe, ModelFile, NativeRoute, Source};

/// The folder both files live in.
const FOLDER: &str = "depth_anything_v2_small";

/// The Hugging Face repository that holds the candle-format conversions, and the revision both
/// files are pinned to.
const REPO: &str = "pcuenq/candle-depth-anything-v2-small";
const REVISION: &str = "11c7cdb8f744281f9264f36ffb541f00e36fe9b8";

/// The depth head (the DPT decoder) and the DINOv2 backbone it reads, both half-precision
/// `.safetensors` at a pinned revision, read as they are: nothing hosted by us. The head is first,
/// because it is the weights file the connection records; the backbone sits beside it.
fn depth_anything_files() -> Vec<ModelFile> {
    vec![
        ModelFile {
            folder: FOLDER.to_owned(),
            filename: "depth_anything_v2_vits.fp16.safetensors".to_owned(),
            size_bytes: 49_595_170,
            sha256: "73e34d35b475bc075ac0b061671e5780800ed734c7eba2f702755f756b60b171".to_owned(),
            source: source("depth_anything_v2_vits.fp16.safetensors"),
            licence: Licence {
                name: "Apache-2.0".to_owned(),
                url: Some("https://huggingface.co/depth-anything/Depth-Anything-V2-Small".to_owned()),
                accept_required: false,
                note: Some(
                    "Depth Anything V2 Small is published by its authors under Apache-2.0. This file is a candle-format conversion of their checkpoint."
                        .to_owned(),
                ),
            },
        },
        ModelFile {
            folder: FOLDER.to_owned(),
            filename: "dinov2_vits14.fp16.safetensors".to_owned(),
            size_bytes: 45_667_104,
            sha256: "ebfaef20f34130315ced79ef159f27acfba48ba271d6a2261483199b02a02983".to_owned(),
            source: source("dinov2_vits14.fp16.safetensors"),
            licence: Licence {
                name: "Apache-2.0".to_owned(),
                url: Some("https://github.com/facebookresearch/dinov2/blob/main/LICENSE".to_owned()),
                accept_required: false,
                note: Some(
                    "The DINOv2 ViT-S/14 backbone is published by Meta under Apache-2.0. This file is a candle-format conversion of their checkpoint."
                        .to_owned(),
                ),
            },
        },
    ]
}

fn source(file: &str) -> Source {
    Source::HuggingFace {
        repo: REPO.to_owned(),
        file: file.to_owned(),
        revision: Some(REVISION.to_owned()),
    }
}

/// Depth Anything V2 Small for Estimate depth. ComfyUI does not run it here: the built-in engine
/// reads two small files and nothing else.
pub(super) fn depth_anything() -> LocalRecipe {
    LocalRecipe {
        label: "Depth Anything V2 Small".to_owned(),
        usable: true,
        workflow: "depth-anything-v2-small".to_owned(),
        comfyui: false,
        models: Vec::new(),
        packs: Vec::new(),
        min_vram_mb: None,
        native: vec![NativeRoute {
            provider: "candle".to_owned(),
            model: "depth-anything-v2-small".to_owned(),
            label: "Depth Anything V2 Small, built in".to_owned(),
            models: depth_anything_files(),
            note: None,
            fast_on_processor: false,
        }],
        components: Vec::new(),
    }
}
