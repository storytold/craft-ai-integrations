//! Built-in model setup must use the files its own runner loads, not the ComfyUI recipe.
use craft_ai_features::{
    InstalledFile, Inventory, Route, State, builtin, plan_native, setup_options,
};

/// With no ComfyUI and no files, the Candle plan lists its own three files, their total size and the
/// one licence to accept, and asks for no node packs.
#[test]
fn candle_without_comfyui_plans_its_three_own_files() {
    let feature = builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    let plan = plan_native(&feature, "candle", &Inventory::default());
    assert_eq!(plan.state, State::NeedsInstall);
    let names: Vec<_> = plan
        .missing_files
        .iter()
        .map(|f| f.filename.as_str())
        .collect();
    assert_eq!(names, ["model.safetensors", "vocab.json", "merges.txt"]);
    assert_eq!(plan.download_bytes, 3_441_325_459);
    assert_eq!(plan.licences.len(), 1);
    assert_eq!(plan.licences[0].name, "Meta SAM License");
    assert!(plan.missing_packs.is_empty());
}

/// When all of the runtime's own files are present the plan is Ready with nothing to download.
#[test]
fn native_files_already_present_are_ready_without_comfyui() {
    let feature = builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    let candle = feature
        .engine_recipe("candle")
        .and_then(|r| r.native.iter().find(|n| n.provider == "candle"))
        .unwrap();
    let inventory = Inventory {
        files: candle
            .models
            .iter()
            .map(|m| InstalledFile {
                folder: m.folder.clone(),
                filename: m.filename.clone(),
                size_bytes: m.size_bytes,
            })
            .collect(),
        ..Inventory::default()
    };
    let plan = plan_native(&feature, "candle", &inventory);
    assert_eq!(plan.state, State::Ready);
    assert_eq!(plan.download_bytes, 0);
    assert!(plan.licences.is_empty());
}

/// The runtime's missing bytes are checked against free disk space, and too little blocks the plan.
#[test]
fn missing_native_files_count_against_free_space() {
    let feature = builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    let inventory = Inventory {
        free_disk_bytes: Some(100),
        ..Inventory::default()
    };
    let plan = plan_native(&feature, "candle", &inventory);
    assert_eq!(plan.state, State::Blocked);
    assert_eq!(plan.blockers.len(), 1);
}

/// An unknown engine gets no plan and no download.
#[test]
fn unfinished_or_unknown_engines_offer_no_download() {
    let feature = builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    let plan = plan_native(&feature, "not-an-engine", &Inventory::default());
    assert_eq!(plan.state, State::NoLocalRoute);
    assert!(plan.missing_files.is_empty());
}

/// The setup options list Candle with its own size, and Burn with the size of SAM 2.1's two
/// published files (its SAM 3 slot names none, so it never offers that download).
#[test]
fn setup_options_offer_candle_with_its_own_size_and_burn_with_sam_2_1s() {
    let options = setup_options(&builtin());
    let candle = options
        .iter()
        .find(|o| o.id == "native:select-subject:candle")
        .unwrap();
    assert!(
        matches!(&candle.route, Route::Native { feature, provider, model }
        if feature == "select-subject" && provider == "candle" && model == "sam3")
    );
    assert_eq!(candle.size_bytes, Some(3_441_325_459));
    assert!(candle.usable);
    let burn = options
        .iter()
        .find(|o| o.id == "native:select-subject:burn")
        .unwrap();
    assert!(matches!(&burn.route, Route::Native { provider, model, .. }
        if provider == "burn" && model == "sam21"));
    assert_eq!(burn.size_bytes, Some(130_129_714));
}

/// A recipe apps cannot run yet offers no native plan, even though its runtime names files.
#[test]
fn a_recipe_apps_cannot_run_yet_offers_no_native_download() {
    let mut feature = builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    for recipe in feature.locals.iter_mut() {
        if recipe.workflow == "sam3-segment" {
            recipe.usable = false;
        }
    }
    let plan = plan_native(&feature, "candle", &Inventory::default());
    assert_eq!(plan.state, State::NoLocalRoute);
    assert!(plan.missing_files.is_empty());
}

/// `select-subject` with a SAM 3 recipe that cannot run listed before the real one, and a later
/// copy of the real one under another workflow id. Only the first usable recipe that lists Candle
/// may serve it, the same one `enable_native_with` and file setup use.
fn unusable_first() -> craft_ai_features::Feature {
    let mut feature = builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    let real = feature
        .locals
        .iter()
        .find(|r| r.workflow == "sam3-segment")
        .unwrap()
        .clone();
    let mut parked = real.clone();
    parked.usable = false;
    parked.workflow = "parked".to_owned();
    let mut later = real.clone();
    later.workflow = "later".to_owned();
    feature.locals = vec![parked, real, later];
    feature
}

/// An unusable recipe in front does not hide the usable one behind it.
#[test]
fn plan_native_uses_the_first_usable_recipe_not_the_first_recipe() {
    let feature = unusable_first();
    let plan = plan_native(&feature, "candle", &Inventory::default());
    assert_eq!(plan.state, State::NeedsInstall);
    assert_eq!(plan.recipe, feature.locals[1].workflow);
    assert_eq!(plan.download_bytes, 3_441_325_459);
}

/// A runtime gets one option however many recipes name it, and the option is the one the planner
/// and the installer act on, so its id is unique and following it cannot fail.
#[test]
fn a_native_runtime_gets_one_option_from_the_first_usable_recipe() {
    let feature = unusable_first();
    let options = setup_options(std::slice::from_ref(&feature));
    let native: Vec<_> = options
        .iter()
        .filter(|o| matches!(o.route, Route::Native { .. }))
        .collect();
    assert_eq!(native.len(), 1);
    assert_eq!(native[0].id, "native:select-subject:candle");
    assert!(native[0].usable);
    assert_eq!(native[0].size_bytes, Some(3_441_325_459));
}

/// With no usable recipe at all, no native option is offered.
#[test]
fn no_native_option_without_a_usable_recipe() {
    let mut feature = unusable_first();
    feature.locals.iter_mut().for_each(|r| r.usable = false);
    let options = setup_options(std::slice::from_ref(&feature));
    assert!(
        options
            .iter()
            .all(|o| !matches!(o.route, Route::Native { .. }))
    );
}

/// Every local model comes before any built-in runtime, whichever tool names each, and the
/// runtimes come before hosted services.
#[test]
fn native_options_follow_every_local_option_of_every_feature() {
    let first = builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    let mut second = first.clone();
    second.id = "another-tool".to_owned();
    second.locals.iter_mut().for_each(|r| r.native.clear());
    let options = setup_options(&[first, second]);
    let kind = |o: &craft_ai_features::SetupOption| match o.route {
        Route::Local { .. } => 0,
        Route::Native { .. } => 1,
        Route::Service { .. } => 2,
    };
    let kinds: Vec<_> = options.iter().map(kind).collect();
    assert!(kinds.contains(&1), "a native option is offered");
    assert!(
        kinds.is_sorted(),
        "locals, then runtimes, then services: {kinds:?}"
    );
}

/// Each built-in engine follows its own recipe: the first usable one that lists it. SAM 2.1 comes
/// first and lists only rten, so rten plans from SAM 2.1 while Candle still plans from SAM 3, and
/// the setup options offer both, each with its own files.
#[test]
fn each_engine_plans_from_the_first_usable_recipe_that_lists_it() {
    let feature = builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    // Burn is listed on SAM 2.1 after rten, so it comes before Candle, which only SAM 3 lists.
    assert_eq!(feature.engines(), ["rten", "burn", "candle"]);
    let none = Inventory::default();
    let rten = plan_native(&feature, "rten", &none);
    assert_eq!(rten.recipe, "sam21-segment");
    let names: Vec<_> = rten
        .missing_files
        .iter()
        .map(|f| f.filename.as_str())
        .collect();
    assert_eq!(names, ["encoder.onnx", "decoder.onnx"]);
    let candle = plan_native(&feature, "candle", &none);
    assert_eq!(candle.recipe, "sam3-segment");
    assert_eq!(candle.download_bytes, 3_441_325_459);
    // Burn also plans from SAM 2.1 (its route is listed there), with the same two files.
    let burn = plan_native(&feature, "burn", &none);
    assert_eq!(burn.recipe, "sam21-segment");
    assert_eq!(burn.missing_files, rten.missing_files);
    let options = setup_options(std::slice::from_ref(&feature));
    let native: Vec<_> = options
        .iter()
        .filter(|o| matches!(o.route, Route::Native { .. }))
        .map(|o| o.id.as_str())
        .collect();
    assert_eq!(
        native,
        [
            "native:select-subject:rten",
            "native:select-subject:burn",
            "native:select-subject:candle"
        ]
    );
}
