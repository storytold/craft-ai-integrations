//! The DeepFilterNet 3 recipe for Isolate voice: one built-in route on rten, the three published
//! ONNX files pinned by size, hash and revision, and no licence to accept.

use craft_ai_core::{CapabilityId, InputRole};
use craft_ai_features::{Feature, Source, builtin};

/// The built-in Isolate voice feature.
fn isolate_voice() -> Feature {
    builtin()
        .into_iter()
        .find(|f| f.id == "isolate-voice")
        .unwrap()
}

/// Isolate voice is `audio.enhance` on a recording, offered to SoundCraft, and run by one recipe
/// with one built-in route, the one the rten engine's `Dfn3Runner` serves.
#[test]
fn isolate_voice_is_audio_enhance_run_built_in_by_deepfilternet3_on_rten() {
    let f = isolate_voice();
    assert_eq!(f.capability, CapabilityId::parse("audio.enhance").unwrap());
    assert_eq!(f.inputs, [InputRole::Source]);
    assert!(f.uses.iter().any(|u| u.app == "SoundCraft"));
    assert!(f.cloud.is_empty(), "no hosted route is claimed");
    let [recipe] = f.locals.as_slice() else {
        panic!("one recipe")
    };
    assert!(recipe.usable && !recipe.comfyui);
    let [route] = recipe.native.as_slice() else {
        panic!("one route")
    };
    assert_eq!(
        (route.provider.as_str(), route.model.as_str()),
        ("rten", "deepfilternet3")
    );
    // the encoder is the weights file the connection records; the decoders sit beside it
    assert_eq!(route.weights().unwrap().filename, "enc.onnx");
    let names: Vec<_> = route.models.iter().map(|m| m.filename.as_str()).collect();
    assert_eq!(names, ["enc.onnx", "erb_dec.onnx", "df_dec.onnx"]);
}

/// Each file is pinned to the exact bytes the DeepFilterNet authors publish: size, SHA-256 and a
/// revision-pinned address, all in one folder, with no licence to accept.
#[test]
fn the_files_are_pinned_to_the_published_export() {
    let f = isolate_voice();
    let route = &f.locals[0].native[0];
    let expected = [
        (
            "enc.onnx",
            1_954_042,
            "7c5399d3da8a50ebef1c1a0ae421b33376aa5e45d0e92df16da7e83c9c131916",
        ),
        (
            "erb_dec.onnx",
            3_292_397,
            "ab669a1d10afe20911728b33053a452071042317a90581092b325da7b2f9d895",
        ),
        (
            "df_dec.onnx",
            3_340_803,
            "23114ce3b0f6464b763ee62f7bb8aab6b2a129a21eabd5bcfe59413db05f278a",
        ),
    ];
    for (m, (name, size, sha)) in route.models.iter().zip(expected) {
        assert_eq!(
            (m.filename.as_str(), m.size_bytes, m.sha256.as_str()),
            (name, size, sha)
        );
        assert_eq!(m.folder, "deepfilternet3_onnx");
        assert!(!m.licence.accept_required);
        let Source::HuggingFace { revision, .. } = &m.source else {
            panic!("a Hugging Face file")
        };
        assert_eq!(
            revision.as_deref().map(str::len),
            Some(40),
            "pinned to a commit"
        );
        assert!(
            m.source
                .url()
                .contains(&format!("/resolve/{}/{name}", revision.as_deref().unwrap()))
        );
    }
}
