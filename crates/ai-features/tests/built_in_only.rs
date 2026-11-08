//! Recipes only a built-in runtime runs: Remove background on InSPyReNet, read by the Burn engine,
//! and on BEN2 + ViTMatte, read by the rten engine. Each is offered and planned as its runtime
//! alone, never as a ComfyUI model, install or plan.
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

/// Both recipes are built in only and name their runner's model; InSPyReNet pins one published
/// file.
#[test]
fn remove_background_runs_inspyrenet_on_burn_from_one_pinned_file() {
    let feature = remove_background();
    assert_eq!(feature.locals.len(), 2);
    assert!(
        feature
            .locals
            .iter()
            .all(|r| r.usable && !r.comfyui && r.models.is_empty() && r.packs.is_empty())
    );
    assert_eq!(feature.engines(), ["burn", "rten"]);
    assert_eq!(feature.comfyui_recipes().count(), 0);
    let recipe = &feature.locals[0];
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

/// BEN2 + ViTMatte reads three pinned ONNX files from one folder, `ben2.onnx` first (the weights
/// file the rten runner is given), each with its size, checksum and licence.
#[test]
fn remove_background_runs_ben2_vitmatte_on_rten_from_three_pinned_files() {
    let feature = remove_background();
    let recipe = &feature.locals[1];
    assert_eq!(recipe.workflow, "ben2-vitmatte-matte");
    assert!(
        recipe.usable && !recipe.comfyui,
        "built in only, never a ComfyUI model"
    );
    assert!(recipe.models.is_empty() && recipe.packs.is_empty());
    let route = &recipe.native[0];
    assert_eq!(
        (route.provider.as_str(), route.model.as_str()),
        ("rten", "ben2-vitmatte")
    );
    assert_eq!(route.weights().unwrap().filename, "ben2.onnx");
    let names: Vec<&str> = route.models.iter().map(|m| m.filename.as_str()).collect();
    assert_eq!(
        names,
        ["ben2.onnx", "vitmatte-small.onnx", "vitmatte-base.onnx"]
    );
    assert!(route.models.iter().all(|m| m.folder == "ben2_vitmatte"));
    assert_eq!(
        route.models.iter().map(|m| m.size_bytes).sum::<u64>(),
        714_189_538
    );
    for m in &route.models {
        assert_eq!(m.sha256.len(), 64, "{}", m.filename);
        assert!(!m.licence.accept_required, "{}", m.filename);
        match &m.source {
            Source::HuggingFace { revision, .. } => assert_eq!(
                revision.as_deref().map(str::len),
                Some(40),
                "{} pinned",
                m.filename
            ),
            Source::Url { .. } => panic!("expected a pinned Hugging Face file"),
        }
    }
}

/// Set up AI lists the two built-in routes and no ComfyUI model for the tool.
#[test]
fn only_the_built_in_route_is_offered() {
    let options = setup_options(&[remove_background()]);
    assert_eq!(options.len(), 2, "{options:?}");
    assert_eq!(options[0].id, "native:remove-background:burn");
    assert!(matches!(&options[0].route, Route::Native { model, .. } if model == "inspyrenet"));
    assert_eq!(options[0].size_bytes, Some(367_120_444));
    assert_eq!(options[1].id, "native:remove-background:rten");
    assert!(matches!(&options[1].route, Route::Native { model, .. } if model == "ben2-vitmatte"));
    assert_eq!(options[1].size_bytes, Some(714_189_538));
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

    let native = plan_native(&feature, "rten", &inventory);
    assert_eq!(native.state, State::NeedsInstall);
    assert_eq!(native.recipe, "ben2-vitmatte-matte");
    assert_eq!(native.download_bytes, 714_189_538);
}
