//! LaMa for Remove objects, run by the built-in rten engine: fills a marked area from its
//! surroundings without a text prompt. The runner is in `craft-ai-engine-rten`'s `lama` module.

use crate::recipe::{Licence, LocalRecipe, ModelFile, NativeRoute, Source};

/// The published ONNX export of big-lama at a pinned revision, read directly by rten with no
/// conversion and nothing hosted by us.
fn lama_files() -> Vec<ModelFile> {
    vec![ModelFile {
        folder: "lama".to_owned(),
        filename: "lama.onnx".to_owned(),
        size_bytes: 208_044_816,
        sha256: "1faef5301d78db7dda502fe59966957ec4b79dd64e16f03ed96913c7a4eb68d6".to_owned(),
        source: Source::HuggingFace {
            repo: "Carve/LaMa-ONNX".to_owned(),
            file: "lama_fp32.onnx".to_owned(),
            revision: Some("c3c0c9e468934d62e79c329e35d82dd09ff8c444".to_owned()),
        },
        licence: Licence {
            name: "Apache-2.0".to_owned(),
            url: Some("https://huggingface.co/Carve/LaMa-ONNX".to_owned()),
            accept_required: false,
            note: Some(
                "An ONNX export of the big-lama weights (advimman/lama), both released under Apache 2.0."
                    .to_owned(),
            ),
        },
    }]
}

/// LaMa for Remove objects, built in only (rten, on the processor).
pub(super) fn lama() -> LocalRecipe {
    LocalRecipe {
        label: "LaMa".to_owned(),
        usable: true,
        workflow: "lama-remove".to_owned(),
        comfyui: false,
        models: Vec::new(),
        packs: Vec::new(),
        min_vram_mb: None,
        native: vec![NativeRoute {
            provider: "rten".to_owned(),
            model: "lama".to_owned(),
            label: "LaMa, built in (on the processor)".to_owned(),
            models: lama_files(),
            note: Some(
                "Fills the marked area from its surroundings without a prompt. Runs locally on the processor."
                    .to_owned(),
            ),
            fast_on_processor: false,
        }],
        components: Vec::new(),
    }
}
