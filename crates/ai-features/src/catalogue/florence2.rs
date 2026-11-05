//! Microsoft's Florence-2-large, the detector that turns the words of a prompt into boxes for SAM 2.1.

use crate::recipe::{Licence, ModelFile, Source};

/// The folder name ComfyUI's Florence 2 add-on shows for the detector, and the graph asks for.
/// `craft_ai_comfyui::FLORENCE2_DETECTOR` must stay equal to it; a test there checks.
pub const FLORENCE2_LARGE_DIR: &str = "Florence-2-large";

/// Every file of Florence-2-large, MIT licensed, pinned to one revision and checked by SHA-256.
/// ComfyUI's Florence 2 add-on lists the folders of `models/LLM`, so the files sit in
/// `LLM/Florence-2-large/`.
pub(super) fn florence2_large_files() -> Vec<ModelFile> {
    const REVISION: &str = "21a599d414c4d928c9032694c424fb94458e3594";
    let file = |leaf: &str, size_bytes: u64, sha256: &str| {
        ModelFile {
        folder: "LLM".to_owned(),
        filename: format!("{FLORENCE2_LARGE_DIR}/{leaf}"),
        size_bytes,
        sha256: sha256.to_owned(),
        source: Source::HuggingFace {
            repo: "microsoft/Florence-2-large".to_owned(),
            file: leaf.to_owned(),
            revision: Some(REVISION.to_owned()),
        },
        licence: Licence {
            name: "MIT".to_owned(),
            url: Some(format!(
                "https://huggingface.co/microsoft/Florence-2-large/blob/{REVISION}/LICENSE"
            )),
            accept_required: false,
            note: Some(
                "Published by Microsoft. The folder also holds Python files that ComfyUI's Florence 2 add-on loads and runs; they are pinned to one revision and checked against SHA-256."
                    .to_owned(),
            ),
        },
    }
    };
    vec![
        file(
            "model.safetensors",
            1_553_563_458,
            "4f38ce741c6b71188fe2b3419a55e11917a8a7b321ae2e63c61da0191b0ebad7",
        ),
        file(
            "config.json",
            2_445,
            "6f8a8f92a74ce18b5c1e5646b4a8222477dd1ac49f31b953e75d6fc3e0f8583a",
        ),
        file(
            "configuration_florence2.py",
            15_119,
            "de2e45a975b3582de05d2f4d963a3e9f9a3d20dccf78d28e0052932a0be93bdf",
        ),
        file(
            "generation_config.json",
            51,
            "30e9865458ecc8ee931eeeb43f44f1d169c5ab95be39e0072142a7a6b8f31990",
        ),
        file(
            "modeling_florence2.py",
            127_455,
            "5162bf465e61b6e29cc113a467630ec3cb56ed8e4d46eb6207157f10fb9b8a24",
        ),
        file(
            "preprocessor_config.json",
            806,
            "2f5921bbc53c7cc04251e1027b45b1cec726276be6db23d1bb40641bfbe2cf29",
        ),
        file(
            "processing_florence2.py",
            48_674,
            "c655782a9e4347965c735ea54cbc4e98fdbc02155ffd1ce2ecd61f42c45eda28",
        ),
        file(
            "tokenizer.json",
            1_355_863,
            "847bbeab6174d66a88898f729d52fa8d355fafe1bea101cf960dd404581df70e",
        ),
        file(
            "tokenizer_config.json",
            34,
            "79ffcf43af8ebda99d165f61d243180da2e2639952e41e71e11611c18770489c",
        ),
        file(
            "vocab.json",
            1_099_884,
            "394fdc63c71aabe0a9b97117f5d62fb5fcc4d59b2b3ea929a3929e6a53217b3c",
        ),
    ]
}
