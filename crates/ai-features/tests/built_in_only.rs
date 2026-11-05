//! A recipe only a built-in runtime runs: Remove background on InSPyReNet, read by the Burn engine.
//! It is offered and planned as that runtime alone, never as a ComfyUI model, install or plan.
use craft_ai_features::{
    Feature, Inventory, Os, Route, SetupRoute, Source, State, SystemProfile, plan, plan_native,
    plans, setup_offer, setup_offer_native, setup_options,
};

fn remove_background() -> Feature {
    craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == "remove-background")
        .unwrap()
}

fn windows() -> SystemProfile {
    SystemProfile {
        os: Os::Windows,
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

/// The one recipe is built in only, names the Burn runner's model, and pins the published file.
#[test]
fn remove_background_runs_inspyrenet_on_burn_from_one_pinned_file() {
    let feature = remove_background();
    assert_eq!(feature.locals.len(), 1);
    let recipe = &feature.locals[0];
    assert!(recipe.usable && !recipe.comfyui);
    assert!(recipe.models.is_empty() && recipe.packs.is_empty());
    assert_eq!(feature.engines(), ["burn"]);
    assert_eq!(feature.comfyui_recipes().count(), 0);
    let route = &recipe.native[0];
    assert_eq!(route.model, "inspyrenet");
    let weights = route.weights().unwrap();
    assert_eq!(weights.filename, "inspyrenet.safetensors");
    assert_eq!(weights.size_bytes, 367_120_444);
    assert_eq!(
        weights.sha256,
        "5c8ab4541f9a960b376a80362b07f1d96262e689c501365e6323faa958de47ea"
    );
    assert!(!weights.licence.accept_required);
    match &weights.source {
        Source::HuggingFace { revision, .. } => {
            assert_eq!(revision.as_deref().map(str::len), Some(40), "pinned");
        }
        Source::Url { .. } => panic!("expected a pinned Hugging Face file"),
    }
}

/// Set up AI lists the built-in route and no ComfyUI model for the tool.
#[test]
fn only_the_built_in_route_is_offered() {
    let options = setup_options(&[remove_background()]);
    assert_eq!(options.len(), 1, "{options:?}");
    assert_eq!(options[0].id, "native:remove-background:burn");
    assert!(matches!(&options[0].route, Route::Native { model, .. } if model == "inspyrenet"));
    assert_eq!(options[0].size_bytes, Some(367_120_444));
    assert!(!setup_options(&craft_ai_features::builtin()).iter().any(
        |o| matches!(&o.route, Route::Local { feature, .. } if feature == "remove-background")
    ));
}

/// No ComfyUI plan or install offer exists for it; the built-in plan and offer download the file.
#[test]
fn it_is_planned_and_installed_as_the_built_in_route_only() {
    let feature = remove_background();
    let inventory = inventory();
    assert_eq!(plan(&feature, &inventory).state, State::NoLocalRoute);
    assert!(plans(&feature, &inventory).is_empty());
    // Not even on a computer without ComfyUI, where a ComfyUI offer would start with installing it.
    let no_comfyui = Inventory {
        comfyui_found: false,
        ..inventory.clone()
    };
    assert!(setup_offer(&feature, &windows(), &no_comfyui, SetupRoute::ComfyUi).is_none());

    let native = plan_native(&feature, "burn", &inventory);
    assert_eq!(native.state, State::NeedsInstall);
    assert_eq!(native.recipe, "inspyrenet-matte");
    assert_eq!(native.download_bytes, 367_120_444);
    let offer = setup_offer_native(&feature, "burn", &windows(), &inventory).unwrap();
    assert_eq!(offer.download_bytes, 367_120_444);
}
