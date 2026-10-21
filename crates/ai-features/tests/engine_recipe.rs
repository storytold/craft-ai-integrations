//! Each built-in engine follows its own recipe: the first usable one that lists it. The catalogue
//! may list engines in different recipes (one recipe for an engine that reads one format, another
//! for an engine that reads a second), so these tests build that shape from the real SAM 3 recipe
//! rather than depend on how the catalogue is ordered today.
use craft_ai_features::{
    Feature, Gpu, Inventory, Os, Route, SetupRoute, State, StepKind, SystemProfile, builtin,
    plan_native, setup_offer, setup_offer_native, setup_options,
};

/// `select-subject` with two recipes: `first-recipe`, which lists only `rten` (its own two files
/// in a folder of their own), then the real SAM 3 recipe, which lists Candle and Burn.
fn two_recipes() -> Feature {
    let mut feature = builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    let sam3 = feature
        .locals
        .iter()
        .find(|r| r.workflow == "sam3-segment")
        .unwrap()
        .clone();
    let mut first = sam3.clone();
    first.workflow = "first-recipe".to_owned();
    let mut route = sam3
        .native
        .iter()
        .find(|n| n.provider == "candle")
        .unwrap()
        .clone();
    route.provider = "rten".to_owned();
    route.model = "first-recipe".to_owned();
    route.models.truncate(2);
    for file in &mut route.models {
        file.folder = "first_recipe_files".to_owned();
    }
    first.native = vec![route];
    feature.locals = vec![first, sam3];
    feature
}

/// A Windows PC with no graphics card.
fn pc() -> SystemProfile {
    SystemProfile {
        os: Os::Windows,
        gpus: Vec::<Gpu>::new(),
        comfyui_found: false,
    }
}

/// The step kinds of an offer, in order.
fn kinds(offer: &craft_ai_features::SetupOffer) -> Vec<StepKind> {
    offer.steps.iter().map(|s| s.kind).collect()
}

/// Each engine plans from the first usable recipe that lists it, and the setup options offer each
/// engine once, with its own files, in the order the engines first appear.
#[test]
fn each_engine_plans_from_the_first_usable_recipe_that_lists_it() {
    let feature = two_recipes();
    assert_eq!(feature.engines(), ["rten", "candle", "burn"]);
    assert_eq!(
        feature.engine_recipe("rten").unwrap().workflow,
        "first-recipe"
    );
    assert_eq!(
        feature.engine_recipe("candle").unwrap().workflow,
        "sam3-segment"
    );
    let none = Inventory::default();
    let rten = plan_native(&feature, "rten", &none);
    assert_eq!(rten.state, State::NeedsInstall);
    assert_eq!(rten.recipe, "first-recipe");
    assert_eq!(rten.missing_files.len(), 2);
    let candle = plan_native(&feature, "candle", &none);
    assert_eq!(candle.recipe, "sam3-segment");
    assert_eq!(candle.download_bytes, 3_441_325_459);
    let options = setup_options(std::slice::from_ref(&feature));
    let native: Vec<_> = options
        .iter()
        .filter(|o| matches!(o.route, Route::Native { .. }))
        .map(|o| o.id.as_str())
        .collect();
    assert_eq!(
        native,
        ["native:select-subject:rten", "native:select-subject:candle"]
    );
}

/// An engine that no usable recipe lists has no recipe, and an unusable recipe lists nothing.
#[test]
fn an_unusable_recipe_lists_no_engine() {
    let mut feature = two_recipes();
    feature.locals[0].usable = false;
    assert_eq!(feature.engines(), ["candle", "burn"]);
    assert!(feature.engine_recipe("rten").is_none());
    let plan = plan_native(&feature, "rten", &Inventory::default());
    assert_eq!(plan.state, State::NoLocalRoute);
}

/// The plain built-in offer (no engine named) is for the first recipe that can run: with the
/// first recipe unable to run, it is SAM 3's first engine with files, Candle, and its three files.
/// Naming an engine still reaches that engine's own recipe.
#[test]
fn a_plain_built_in_offer_skips_an_unusable_first_recipe() {
    let mut feature = two_recipes();
    let empty = Inventory::default();
    let plain = setup_offer(&feature, &pc(), &empty, SetupRoute::BuiltIn).unwrap();
    assert_eq!(
        kinds(&plain),
        [StepKind::Model; 2],
        "the first recipe's files"
    );
    feature.locals[0].usable = false;
    let plain = setup_offer(&feature, &pc(), &empty, SetupRoute::BuiltIn).unwrap();
    assert_eq!(kinds(&plain), [StepKind::Model; 3]);
    assert_eq!(plain.download_bytes, 3_441_325_459);
    feature.locals[0].usable = true;
    let candle = setup_offer_native(&feature, "candle", &pc(), &empty).unwrap();
    assert_eq!(candle.download_bytes, 3_441_325_459);
    let rten = setup_offer_native(&feature, "rten", &pc(), &empty).unwrap();
    assert_eq!(kinds(&rten), [StepKind::Model; 2]);
}
