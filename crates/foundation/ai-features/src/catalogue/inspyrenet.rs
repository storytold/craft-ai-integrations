//! InSPyReNet (Swin-B, Plus Ultra) for Remove background, run by the built-in Burn engine only.

use crate::recipe::{Licence, LocalRecipe, ModelFile, NativeRoute, Source};

/// The author's checkpoint as a `.safetensors` file at a pinned revision. Its 1,625 tensors were
/// checked one by one against the author's own `ckpt_base.pth` (plemeri/InSPyReNet on Hugging
/// Face). The engine reads this file as it is: no conversion, nothing hosted by us.
fn inspyrenet_files() -> Vec<ModelFile> {
    const FILE: &str = "inspyrenet.safetensors";
    vec![ModelFile {
        folder: "inspyrenet".to_owned(),
        filename: FILE.to_owned(),
        size_bytes: 367_120_444,
        sha256: "5c8ab4541f9a960b376a80362b07f1d96262e689c501365e6323faa958de47ea".to_owned(),
        source: Source::HuggingFace {
            repo: "1038lab/inspyrenet".to_owned(),
            file: FILE.to_owned(),
            revision: Some("0f5946638dcbc7dbc1e56306b823959958ac0ae0".to_owned()),
        },
        licence: Licence {
            name: "MIT, as declared by the author of InSPyReNet".to_owned(),
            url: Some("https://github.com/plemeri/InSPyReNet/blob/main/LICENSE".to_owned()),
            accept_required: false,
            note: Some(
                "This file is a format conversion of the author's checkpoint, identical tensor for tensor. The publisher of the conversion states no licence of its own."
                    .to_owned(),
            ),
        },
    }]
}

/// InSPyReNet for Remove background. ComfyUI does not run it: a node pack and a workflow would be
/// far more to set up than the one file the built-in engine reads.
pub(super) fn inspyrenet() -> LocalRecipe {
    LocalRecipe {
        label: "InSPyReNet".to_owned(),
        usable: true,
        workflow: "inspyrenet-matte".to_owned(),
        comfyui: false,
        models: Vec::new(),
        packs: Vec::new(),
        min_vram_mb: None,
        native: vec![NativeRoute {
            provider: "burn".to_owned(),
            model: "inspyrenet".to_owned(),
            label: "InSPyReNet, built in (any graphics card)".to_owned(),
            models: inspyrenet_files(),
            note: None,
            // Seconds a picture on the processor, not measured as quick (docs/BURN-ENGINE.md).
            fast_on_processor: false,
        }],
        components: Vec::new(),
    }
}
