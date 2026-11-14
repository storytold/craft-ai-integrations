//! Estimate depth: one built-in recipe, Depth Anything V2 Small on the Candle engine, read from
//! two pinned files in one folder. It is offered and planned as that runtime alone, never as a
//! ComfyUI model, install or plan.
use craft_ai_features::{
    Feature, Inventory, Os, Route, SetupRoute, Source, State, SystemProfile, plan, plan_native,
    plans, setup_offer, setup_offer_native, setup_options,
};

const HEAD: &str = "depth_anything_v2_vits.fp16.safetensors";
const BACKBONE: &str = "dinov2_vits14.fp16.safetensors";
const TOTAL_BYTES: u64 = 49_595_170 + 45_667_104;

fn estimate_depth() -> Feature {
    craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == "estimate-depth")
        .unwrap()
}

fn linux() -> SystemProfile {
    SystemProfile {
        os: Os::Linux,
        gpus: Vec::new(),
        comfyui_found: false,
    }
}

/// Plenty of space, ComfyUI present, nothing downloaded.
fn inventory() -> Inventory {
    Inventory {
        comfyui_found: true,
        free_disk_bytes: Some(500_000_000_000),
        ..Inventory::default()
    }
}

/// The tool takes a picture and answers with a depth map, and PhotoCraft names its use.
#[test]
fn the_tool_takes_a_picture_and_answers_with_depth() {
    let feature = estimate_depth();
    assert_eq!(feature.capability.as_str(), "image.depth");
    let inputs: Vec<&str> = feature.inputs.iter().map(|i| i.as_str()).collect();
    assert_eq!(inputs, ["source"]);
    assert!(feature.uses.iter().any(|u| u.app == "PhotoCraft"));
    assert!(feature.cloud.is_empty(), "no hosted route");
}

/// One recipe, built in only, naming the runner's engine and model.
#[test]
fn it_is_one_built_in_recipe_on_candle() {
    let feature = estimate_depth();
    assert_eq!(feature.locals.len(), 1);
    let recipe = &feature.locals[0];
    assert_eq!(recipe.workflow, "depth-anything-v2-small");
    assert!(recipe.usable && !recipe.comfyui);
    assert!(recipe.models.is_empty() && recipe.packs.is_empty());
    assert_eq!(feature.engines(), ["candle"]);
    assert_eq!(feature.comfyui_recipes().count(), 0);
    let route = &recipe.native[0];
    assert_eq!(
        (route.provider.as_str(), route.model.as_str()),
        ("candle", "depth-anything-v2-small")
    );
    assert_eq!(route.label, "Depth Anything V2 Small, built in");
    assert!(!route.fast_on_processor, "nothing was measured as quick");
}

/// Two pinned files in one folder, the head first (the weights file the runner is given), each
/// with its size, checksum and licence.
#[test]
fn it_reads_two_pinned_files_from_one_folder_head_first() {
    let route = &estimate_depth().locals[0].native[0];
    assert_eq!(route.weights().unwrap().filename, HEAD);
    let names: Vec<&str> = route.models.iter().map(|m| m.filename.as_str()).collect();
    assert_eq!(names, [HEAD, BACKBONE]);
    assert!(
        route
            .models
            .iter()
            .all(|m| m.folder == "depth_anything_v2_small")
    );
    assert_eq!(
        route.models.iter().map(|m| m.size_bytes).sum::<u64>(),
        TOTAL_BYTES
    );
    assert_eq!(
        route.models[0].sha256,
        "73e34d35b475bc075ac0b061671e5780800ed734c7eba2f702755f756b60b171"
    );
    assert_eq!(
        route.models[1].sha256,
        "ebfaef20f34130315ced79ef159f27acfba48ba271d6a2261483199b02a02983"
    );
    for m in &route.models {
        assert_eq!(m.licence.name, "Apache-2.0", "{}", m.filename);
        assert!(m.licence.url.is_some(), "{}", m.filename);
        assert!(!m.licence.accept_required, "{}", m.filename);
        assert!(!route.needs_agreement());
        match &m.source {
            Source::HuggingFace { repo, revision, .. } => {
                assert_eq!(repo, "pcuenq/candle-depth-anything-v2-small");
                assert_eq!(
                    revision.as_deref(),
                    Some("11c7cdb8f744281f9264f36ffb541f00e36fe9b8"),
                    "{} pinned",
                    m.filename
                );
            }
            Source::Url { .. } => panic!("expected a pinned Hugging Face file"),
        }
    }
}

/// Set up AI lists the built-in route and no ComfyUI model for the tool.
#[test]
fn only_the_built_in_route_is_offered() {
    let options = setup_options(&[estimate_depth()]);
    assert_eq!(options.len(), 1, "{options:?}");
    assert_eq!(options[0].id, "native:estimate-depth:candle");
    assert!(
        matches!(&options[0].route, Route::Native { model, .. } if model == "depth-anything-v2-small")
    );
    assert_eq!(options[0].size_bytes, Some(TOTAL_BYTES));
    assert!(
        !setup_options(&craft_ai_features::builtin()).iter().any(
            |o| matches!(&o.route, Route::Local { feature, .. } if feature == "estimate-depth")
        )
    );
}

/// No ComfyUI plan or install offer exists for it; the built-in plan and offer download the two
/// files.
#[test]
fn it_is_planned_and_installed_as_the_built_in_route_only() {
    let feature = estimate_depth();
    let inventory = inventory();
    assert_eq!(plan(&feature, &inventory).state, State::NoLocalRoute);
    assert!(plans(&feature, &inventory).is_empty());
    let no_comfyui = Inventory {
        comfyui_found: false,
        ..inventory.clone()
    };
    assert!(setup_offer(&feature, &linux(), &no_comfyui, SetupRoute::ComfyUi).is_none());

    let native = plan_native(&feature, "candle", &inventory);
    assert_eq!(native.state, State::NeedsInstall);
    assert_eq!(native.recipe, "depth-anything-v2-small");
    assert_eq!(native.download_bytes, TOTAL_BYTES);
    let offer = setup_offer_native(&feature, "candle", &linux(), &inventory).unwrap();
    assert_eq!(offer.download_bytes, TOTAL_BYTES);
}
