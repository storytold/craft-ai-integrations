//! The SAM 2.1 recipe for Select subject: its checkpoint, the Florence-2-large detector it needs to
//! understand words, and the disclosure of every part its workflow runs.

use craft_ai_features::{
    FLORENCE2_LARGE_DIR, Feature, InstalledFile, Inventory, LocalRecipe, Source, State, builtin,
    plan, plans,
};

/// The built-in Select subject feature.
fn select_subject() -> Feature {
    builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap()
}

/// The SAM 2.1 recipe of `f`, found by its workflow id.
fn sam21(f: &Feature) -> &LocalRecipe {
    f.locals
        .iter()
        .find(|r| r.workflow == "sam21-segment")
        .unwrap()
}

/// Select subject lists SAM 2.1 first, then SAM 3, and both are marked usable by apps. SAM 2.1
/// leads because of its licence: Meta releases it under Apache 2.0, while SAM 3 is under the SAM
/// License behind a gated download. The first recipe is the one a plain plan, `enable_local` and
/// the plain built-in offer use, so it is the starter a newcomer is offered.
#[test]
fn select_subject_offers_sam_2_1_then_sam_3_and_apps_can_run_both() {
    let f = select_subject();
    let listed: Vec<_> = f
        .locals
        .iter()
        .map(|r| (r.label.as_str(), r.workflow.as_str(), r.usable))
        .collect();
    assert_eq!(
        listed,
        [
            ("SAM 2.1", "sam21-segment", true),
            ("SAM 3", "sam3-segment", true)
        ]
    );
    assert_eq!(
        f.first_usable_recipe().map(|r| r.workflow.as_str()),
        Some("sam21-segment")
    );
    assert_eq!(plan(&f, &Inventory::default()).recipe, "sam21-segment");
    assert!(
        f.locals[0]
            .models
            .iter()
            .all(|m| !m.licence.accept_required),
        "the starter asks no one to accept a licence"
    );
}

/// The SAM 2.1 checkpoint pins the exact filename, size, hash and revision-pinned URL, and the
/// recipe names the two node packs it needs.
#[test]
fn the_checkpoint_is_the_exact_file_its_loader_reads_at_default_precision() {
    let f = select_subject();
    let m = &sam21(&f).models[0];
    assert_eq!(
        (m.folder.as_str(), m.filename.as_str()),
        ("sam2", "sam2.1_hiera_large-fp16.safetensors")
    );
    assert_eq!(m.size_bytes, 448_999_828);
    assert_eq!(
        m.sha256,
        "278af5296fcc153aa94645a4981e93fe8b7a936a9fa27dbd71e8c850df6d7038"
    );
    assert_eq!(
        m.source.url(),
        "https://huggingface.co/Kijai/sam2-safetensors/resolve/f885607d88bb3f9145efa49c3e3c50a9e5bf13eb/sam2.1_hiera_large-fp16.safetensors"
    );
    let packs: Vec<_> = sam21(&f)
        .packs
        .iter()
        .flat_map(|p| p.folder_names.iter().map(String::as_str))
        .collect();
    assert_eq!(packs, ["comfyui-segment-anything-2", "comfyui-florence2"]);
}

/// The detector is ten Florence-2-large files in the shared folder, MIT with no acceptance step,
/// all from one 40-character revision, and the recipe's total size is checkpoint plus detector.
#[test]
fn the_detector_is_florence_2_large_file_by_file_at_one_pinned_revision() {
    let f = select_subject();
    let detector: Vec<_> = sam21(&f)
        .models
        .iter()
        .filter(|m| m.folder == "LLM")
        .collect();
    assert_eq!(detector.len(), 10);
    assert!(
        detector
            .iter()
            .all(|m| m.filename.starts_with(&format!("{FLORENCE2_LARGE_DIR}/")))
    );
    assert!(
        detector
            .iter()
            .all(|m| m.licence.name == "MIT" && !m.licence.accept_required)
    );
    assert!(detector.iter().all(|m| matches!(&m.source,
        Source::HuggingFace { repo, revision: Some(rev), .. }
            if repo == "microsoft/Florence-2-large" && rev.len() == 40)));
    let total: u64 = sam21(&f).models.iter().map(|m| m.size_bytes).sum();
    assert_eq!(total, 448_999_828 + 1_556_213_789);
}

/// Each part of the workflow is listed with a link and a role, none requires acceptance, the
/// add-on's own download is disclosed, and SAM 3 (not reviewed) lists no parts.
#[test]
fn every_part_of_the_workflow_is_disclosed_and_none_of_it_gates_anything() {
    let f = select_subject();
    let parts = &sam21(&f).components;
    let labels: Vec<_> = parts.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(
        labels,
        [
            "SAM 2.1 checkpoint",
            "ComfyUI segment anything 2 add-on",
            "ComfyUI Florence 2 add-on",
            "Florence-2-large detector"
        ]
    );
    for part in parts {
        let url = part.licence.url.as_deref().unwrap_or_default();
        assert!(url.starts_with("https://"), "{} needs a link", part.label);
        assert!(
            !part.licence.accept_required,
            "{} is disclosure only",
            part.label
        );
        assert!(!part.role.is_empty());
    }
    let note = parts[1].licence.note.as_deref().unwrap_or_default();
    assert!(
        note.contains("downloads it from Hugging Face by itself"),
        "the add-on's own download must be disclosed"
    );
    // The readiness scan compares names and sizes only, so the note must not claim a hash check.
    assert!(!note.contains("is checked before"), "{note}");
    // A recipe whose parts have not been reviewed lists none rather than implying a clean bill.
    let sam3 = f
        .locals
        .iter()
        .find(|r| r.workflow == "sam3-segment")
        .unwrap();
    assert!(sam3.components.is_empty());
}

/// SAM 2.1 plans as Ready only with the checkpoint, every detector file and both node packs; a
/// different Florence variant leaves the detector missing and its bytes still to download.
#[test]
fn sam_2_1_is_ready_only_with_the_checkpoint_and_every_detector_file() {
    let f = select_subject();
    let mut inventory = Inventory {
        comfyui_found: true,
        files: sam21(&f)
            .models
            .iter()
            .map(|m| InstalledFile {
                folder: m.folder.clone(),
                filename: m.filename.clone(),
                size_bytes: m.size_bytes,
            })
            .collect(),
        node_folders: vec![
            "comfyui-segment-anything-2".into(),
            "comfyui-florence2".into(),
        ],
        ..Inventory::default()
    };
    let of_sam21 = |inv: &Inventory| {
        plans(&f, inv)
            .into_iter()
            .find(|p| p.recipe == "sam21-segment")
            .unwrap()
    };
    assert_eq!(of_sam21(&inventory).state, State::Ready);
    // Another Florence variant (the Flux fine-tune, say) does not count as the detector.
    inventory
        .files
        .retain(|f| f.filename != "Florence-2-large/model.safetensors");
    inventory.files.push(InstalledFile {
        folder: "LLM".into(),
        filename: "Florence-2-Flux-Large/model.safetensors".into(),
        size_bytes: 1_553_563_458,
    });
    let missing = of_sam21(&inventory);
    assert_eq!(missing.state, State::NeedsInstall);
    assert_eq!(missing.missing_files.len(), 1);
    assert_eq!(missing.download_bytes, 1_553_563_458);
}
