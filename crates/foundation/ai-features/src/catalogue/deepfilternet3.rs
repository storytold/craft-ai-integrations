//! DeepFilterNet 3 for Isolate voice, run by the built-in rten engine only.

use crate::recipe::{Licence, LocalRecipe, ModelFile, NativeRoute, Source};

/// The publisher's ONNX export of DeepFilterNet 3, three files side by side, `enc.onnx` first (the
/// weights file the connection records). The publisher ships them only inside
/// `models/DeepFilterNet3_onnx.tar.gz` in Rikorose/DeepFilterNet; this mirror at a pinned revision
/// holds the same three files one by one, and each was checked byte for byte against that archive
/// by SHA-256. The engine reads them as they are: no conversion, nothing hosted by us.
///
/// Licence of the weights, as read on 02/11/2026: the repository's root `LICENSE` is MIT OR
/// Apache-2.0 and makes no exception for `models/`; the authors' own `libDF` (manifest licence
/// `MIT/Apache-2.0`) embeds `models/DeepFilterNet3_onnx.tar.gz` with `include_bytes!` under its
/// default `default-model` feature, so their release builds ship it under that licence. Nothing
/// states a licence for the weights in so many words.
fn deepfilternet3_files() -> Vec<ModelFile> {
    const REVISION: &str = "191b102108bf10180909258809dcb3d2a0e59b46";
    let file = |name: &str, size_bytes: u64, sha256: &str| {
        ModelFile {
        folder: "deepfilternet3_onnx".to_owned(),
        filename: name.to_owned(),
        size_bytes,
        sha256: sha256.to_owned(),
        source: Source::HuggingFace {
            repo: "tonythethompson/DeepFilterNet3-ONNX".to_owned(),
            file: name.to_owned(),
            revision: Some(REVISION.to_owned()),
        },
        licence: Licence {
            name: "MIT OR Apache-2.0, the DeepFilterNet repository's licence".to_owned(),
            url: Some("https://github.com/Rikorose/DeepFilterNet#license".to_owned()),
            accept_required: false,
            note: Some(
                "Identical, byte for byte, to the ONNX export the DeepFilterNet authors publish in their repository's models folder. That repository is licensed MIT OR Apache-2.0 with no exception for the models, and the authors' own Rust library (libDF, under the same licence) builds this very archive in by default. No statement names the weights separately. The mirror declares Apache-2.0."
                    .to_owned(),
            ),
        },
    }
    };
    vec![
        file(
            "enc.onnx",
            1_954_042,
            "7c5399d3da8a50ebef1c1a0ae421b33376aa5e45d0e92df16da7e83c9c131916",
        ),
        file(
            "erb_dec.onnx",
            3_292_397,
            "ab669a1d10afe20911728b33053a452071042317a90581092b325da7b2f9d895",
        ),
        file(
            "df_dec.onnx",
            3_340_803,
            "23114ce3b0f6464b763ee62f7bb8aab6b2a129a21eabd5bcfe59413db05f278a",
        ),
    ]
}

/// DeepFilterNet 3 for Isolate voice. ComfyUI does not run it: the built-in engine reads the three
/// small files directly.
pub(super) fn deepfilternet3() -> LocalRecipe {
    LocalRecipe {
        label: "DeepFilterNet 3".to_owned(),
        usable: true,
        workflow: "deepfilternet3-enhance".to_owned(),
        comfyui: false,
        models: Vec::new(),
        packs: Vec::new(),
        min_vram_mb: None,
        native: vec![NativeRoute {
            provider: "rten".to_owned(),
            model: "deepfilternet3".to_owned(),
            label: "DeepFilterNet 3, built in (the processor, on any system)".to_owned(),
            models: deepfilternet3_files(),
            note: Some("48 kHz recordings. Each channel is cleaned on its own.".to_owned()),
            // About 55 times faster than real time on one desktop processor: 59 s of stereo in about
            // 1 s. One machine, one recording; not a benchmark.
            fast_on_processor: true,
        }],
        components: Vec::new(),
    }
}
