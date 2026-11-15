//! BEN2 + ViTMatte for Remove background, run by the built-in rten engine: BEN2 finds the
//! subject, the small and base ViTMatte matte every edge pixel together. The recipe is in
//! `craft-ai-engine-rten`'s `matte` module.

use crate::recipe::{Licence, LocalRecipe, ModelFile, NativeRoute, Source};

/// The publishers' ONNX exports at pinned revisions, read as they are: no conversion, nothing
/// hosted by us. All three sit in one folder, `ben2.onnx` first (the weights file the connection
/// records).
fn ben2_vitmatte_files() -> Vec<ModelFile> {
    const FOLDER: &str = "ben2_vitmatte";
    vec![
        ModelFile {
            folder: FOLDER.to_owned(),
            filename: "ben2.onnx".to_owned(),
            size_bytes: 222_932_053,
            sha256: "22cea62108ff53b7ccc20f7a008bf30494228d84b1687f29ecbe76936a998101".to_owned(),
            source: Source::HuggingFace {
                repo: "PramaLLC/BEN2".to_owned(),
                file: "BEN2_Base.onnx".to_owned(),
                revision: Some("e48a20765fb421d19dcdb0bf3cc61e802ca5ec8f".to_owned()),
            },
            licence: Licence {
                name: "MIT".to_owned(),
                url: Some("https://huggingface.co/PramaLLC/BEN2".to_owned()),
                accept_required: false,
                note: Some(
                    "The ONNX export published by the authors of BEN2, under MIT.".to_owned(),
                ),
            },
        },
        ModelFile {
            folder: FOLDER.to_owned(),
            filename: "vitmatte-small.onnx".to_owned(),
            size_bytes: 103_885_865,
            sha256: "a1cf48234c369faa3ea1711981d961fe1ec71f51e593f9d6553aa5a0e7d557e3".to_owned(),
            source: Source::HuggingFace {
                repo: "Xenova/vitmatte-small-distinctions-646".to_owned(),
                file: "onnx/model.onnx".to_owned(),
                revision: Some("358d428c452e5e0cd52955011a8b51944731d28e".to_owned()),
            },
            licence: vitmatte_licence("hustvl/vitmatte-small-distinctions-646"),
        },
        ModelFile {
            folder: FOLDER.to_owned(),
            filename: "vitmatte-base.onnx".to_owned(),
            size_bytes: 387_371_620,
            sha256: "f6978437f5068849bcbf49b1f4e37b90aaca5155744e9fde4e6c689f70c2b9ee".to_owned(),
            source: Source::HuggingFace {
                repo: "Xenova/vitmatte-base-composition-1k".to_owned(),
                file: "onnx/model.onnx".to_owned(),
                revision: Some("1290b014b994e95ca1b9dd9c5f72c3b6d5b7236a".to_owned()),
            },
            licence: vitmatte_licence("hustvl/vitmatte-base-composition-1k"),
        },
    ]
}

fn vitmatte_licence(original: &str) -> Licence {
    Licence {
        name: "Apache-2.0, ONNX conversion".to_owned(),
        url: Some(format!("https://huggingface.co/{original}")),
        accept_required: false,
        note: Some(
            "This file is an ONNX conversion of the ViTMatte weights, which their authors release under Apache 2.0. The publisher of the conversion states no licence of its own."
                .to_owned(),
        ),
    }
}

/// BEN2 + ViTMatte for Remove background, built in only (rten, on the processor).
pub(super) fn ben2_vitmatte() -> LocalRecipe {
    LocalRecipe {
        label: "BEN2 + ViTMatte".to_owned(),
        usable: true,
        workflow: "ben2-vitmatte-matte".to_owned(),
        comfyui: false,
        models: Vec::new(),
        packs: Vec::new(),
        min_vram_mb: None,
        native: vec![NativeRoute {
            provider: "rten".to_owned(),
            model: "ben2-vitmatte".to_owned(),
            label: "BEN2 + ViTMatte, built in (on the processor)".to_owned(),
            models: ben2_vitmatte_files(),
            note: Some(
                "Edges and hair matted pixel by pixel. Slow on the processor: about a minute for a 1600 × 1200 picture on one Apple Silicon Mac, longer as the subject's edge grows."
                    .to_owned(),
            ),
            fast_on_processor: false,
        }],
        components: Vec::new(),
    }
}
