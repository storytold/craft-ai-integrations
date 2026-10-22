//! The built-in runtimes read their own checkpoint, not the ComfyUI one. These pin that the
//! catalogue says so with complete, checkable data.
use craft_ai_features::{Feature, Source};

/// The built-in `select-subject` feature.
fn select_subject() -> Feature {
    craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap()
}

/// Candle's recipe is SAM 3, and its runtime names its own weights and tokenizer files in one
/// folder, each pinned, hashed and behind the licence, not the ComfyUI checkpoint.
#[test]
fn every_built_in_sam3_runtime_names_its_own_complete_files() {
    let feature = select_subject();
    let recipe = feature.engine_recipe("candle").unwrap();
    assert_eq!(recipe.workflow, "sam3-segment");
    let comfy: Vec<&str> = recipe.models.iter().map(|m| m.filename.as_str()).collect();
    for route in recipe.native.iter().filter(|r| r.provider == "candle") {
        let weights = route.weights().unwrap();
        assert_eq!(weights.filename, "model.safetensors", "{}", route.provider);
        assert!(!comfy.contains(&weights.filename.as_str()));
        // The weights and the tokenizer files must sit in one folder: the loader finds them there.
        assert!(route.models.iter().all(|m| m.folder == weights.folder));
        let names: Vec<&str> = route.models.iter().map(|m| m.filename.as_str()).collect();
        assert!(names.contains(&"vocab.json") && names.contains(&"merges.txt"));
        for m in &route.models {
            assert!(m.size_bytes > 0, "{}", m.filename);
            assert_eq!(m.sha256.len(), 64, "{}", m.filename);
            assert!(
                m.sha256
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            );
            assert!(m.licence.accept_required);
            match &m.source {
                Source::HuggingFace { revision, .. } => {
                    assert_eq!(revision.as_deref().map(str::len), Some(40), "pinned");
                }
                Source::Url { .. } => panic!("expected a pinned Hugging Face file"),
            }
        }
        assert!(route.total_bytes() > 3_000_000_000);
    }
}

/// A route with its files cleared has no weights file and nothing to download.
#[test]
fn a_route_without_files_of_its_own_has_no_weights() {
    let feature = select_subject();
    let mut route = feature.locals.into_iter().next().unwrap().native[0].clone();
    route.models.clear();
    assert!(route.weights().is_none());
    assert_eq!(route.total_bytes(), 0);
}

/// SAM 3's Burn slot still names no files, because no SAM 3 code exists for that engine and a
/// route that cannot run must not offer a 3.4 GB download.
#[test]
fn a_runtime_with_no_code_yet_offers_no_download() {
    let feature = select_subject();
    let sam3 = feature
        .locals
        .iter()
        .find(|r| r.workflow == "sam3-segment")
        .unwrap();
    let burn = sam3.native.iter().find(|r| r.provider == "burn").unwrap();
    assert!(burn.models.is_empty() && burn.weights().is_none());
    assert_eq!(burn.total_bytes(), 0);
}

/// SAM 2.1's Burn route reads the same two published files as rten, listed after it: one download
/// serves both, the encoder is the weights file, and Burn is never first. The files are not hosted
/// converted copies; the engine prepares its own on the computer.
#[test]
fn burns_sam_2_1_route_shares_the_published_files_and_comes_after_rten() {
    let feature = select_subject();
    let recipe = feature.engine_recipe("burn").unwrap();
    assert_eq!(recipe.workflow, "sam21-segment");
    let providers: Vec<_> = recipe.native.iter().map(|r| r.provider.as_str()).collect();
    assert_eq!(providers, ["rten", "burn"]);
    let (rten, burn) = (&recipe.native[0], &recipe.native[1]);
    assert_eq!(burn.model, "sam21");
    assert_eq!(burn.models, rten.models);
    assert_eq!(burn.weights().unwrap().filename, "encoder.onnx");
    assert_eq!(burn.total_bytes(), 130_129_714);
    assert!(burn.models.iter().all(|f| f.filename.ends_with(".onnx")));
}
