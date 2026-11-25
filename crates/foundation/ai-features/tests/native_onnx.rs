//! A built-in engine that reads ONNX files (rten) is described by the catalogue the same way one
//! that reads safetensors files is: the first weights file is the one the connection records.

use craft_ai_features::{Licence, ModelFile, NativeRoute, Source};

/// A one-byte catalogue file called `name` from a made-up source.
fn file(name: &str) -> ModelFile {
    ModelFile {
        folder: "x".to_owned(),
        filename: name.to_owned(),
        size_bytes: 1,
        sha256: "0".repeat(64),
        source: Source::HuggingFace {
            repo: "a/b".to_owned(),
            file: name.to_owned(),
            revision: None,
        },
        licence: Licence {
            name: "test".to_owned(),
            url: None,
            accept_required: false,
            note: None,
        },
    }
}

/// A route for the `rten` engine listing `files` in the order given.
fn route(files: &[&str]) -> NativeRoute {
    NativeRoute {
        provider: "rten".to_owned(),
        model: "m".to_owned(),
        label: "m".to_owned(),
        models: files.iter().map(|f| file(f)).collect(),
        note: None,
        fast_on_processor: false,
    }
}

/// An `.onnx` file counts as weights, in any case, and the first weights file in the list is the
/// one recorded.
#[test]
fn an_onnx_file_is_a_weights_file_and_the_first_one_wins() {
    let r = route(&[
        "README.md",
        "Encoder.ONNX",
        "model.safetensors",
        "decoder.onnx",
    ]);
    assert_eq!(r.weights().unwrap().filename, "Encoder.ONNX");
    let r = route(&["model.safetensors", "encoder.onnx"]);
    assert_eq!(r.weights().unwrap().filename, "model.safetensors");
}

/// Tokenizer files and an empty list give no weights file.
#[test]
fn files_that_are_not_weights_give_none() {
    assert!(route(&["vocab.json", "merges.txt"]).weights().is_none());
    assert!(route(&[]).weights().is_none());
}

/// The SAM 2.1 recipe gives rten the encoder and decoder, each pinned by revision, size and
/// SHA-256, with no licence gate.
#[test]
fn the_sam21_recipe_offers_the_publishers_onnx_files_to_the_rten_engine() {
    let feature = craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    let recipe = feature
        .locals
        .iter()
        .find(|l| l.workflow == "sam21-segment")
        .unwrap();
    let route = recipe.native.iter().find(|n| n.provider == "rten").unwrap();
    assert_eq!(route.model, "sam21");
    assert_eq!(route.weights().unwrap().filename, "encoder.onnx");
    let names: Vec<_> = route.models.iter().map(|m| m.filename.as_str()).collect();
    assert_eq!(names, ["encoder.onnx", "decoder.onnx"]);
    // Pinned and checked: a revision, a size and a SHA-256 for each file, and no licence gate.
    for m in &route.models {
        assert!(
            matches!(&m.source, Source::HuggingFace { revision: Some(r), .. } if r.len() == 40)
        );
        assert_eq!(m.sha256.len(), 64);
        assert!(m.size_bytes > 1_000_000);
        assert!(!m.licence.accept_required);
    }
    assert_eq!(route.total_bytes(), 109_473_919 + 20_655_795);
}
