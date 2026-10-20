//! A tool may offer several local models. Each is planned, offered and named on its own, so one
//! model's files, size or status never stand in for another's.

use craft_ai_features::{
    Feature, InstalledFile, Inventory, LocalRecipe, Route, State, builtin, plan, plans,
    setup_options,
};

/// Select subject with a second, made-up recipe after SAM 3 that needs a different file.
fn two_models() -> Feature {
    let mut f = builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .unwrap();
    // Start from SAM 3 alone, whatever else the catalogue lists.
    f.locals.truncate(1);
    let mut other: LocalRecipe = f.locals[0].clone();
    other.label = "Other".into();
    other.workflow = "other-segment".into();
    other.models.truncate(1);
    other.models[0].folder = "other".into();
    other.models[0].filename = "other.safetensors".into();
    other.models[0].size_bytes = 1_000;
    other.usable = false;
    f.locals.push(other);
    f
}

/// ComfyUI with SAM 3's files and node pack, and nothing of the second model.
fn have_sam3() -> Inventory {
    Inventory {
        comfyui_found: true,
        files: vec![InstalledFile {
            folder: "sam3".into(),
            filename: "sam3.safetensors".into(),
            size_bytes: 3_449_860_632,
        }],
        node_folders: vec!["comfyui-sam3".into()],
        vram_mb: Some(24_576),
        free_disk_bytes: Some(u64::MAX),
    }
}

#[test]
fn each_recipe_gets_its_own_plan_named_by_its_workflow_in_catalogue_order() {
    let all = plans(&two_models(), &have_sam3());
    let names: Vec<_> = all.iter().map(|p| p.recipe.as_str()).collect();
    assert_eq!(names, ["sam3-segment", "other-segment"]);
    assert!(all.iter().all(|p| p.feature == "select-subject"));
    assert_eq!(all[0].state, State::Ready);
    assert_eq!(all[1].state, State::NeedsInstall);
    // The second plan lists only its own file and size, never SAM 3's.
    assert_eq!(all[1].missing_files.len(), 1);
    assert_eq!(all[1].missing_files[0].filename, "other.safetensors");
    assert_eq!(all[1].download_bytes, 1_000);
}

#[test]
fn the_single_plan_is_about_the_first_recipe_and_says_so() {
    let p = plan(&two_models(), &have_sam3());
    assert_eq!(p.recipe, "sam3-segment");
    assert_eq!(p.state, State::Ready);
}

#[test]
fn without_comfyui_every_recipe_says_so() {
    let none = Inventory::default();
    let all = plans(&two_models(), &none);
    assert_eq!(all.len(), 2);
    assert!(all.iter().all(|p| p.state == State::NoRuntime));
    assert_eq!(all[1].recipe, "other-segment");
}

#[test]
fn a_tool_with_no_local_model_has_no_plans_and_an_unnamed_plan() {
    let fill = builtin().into_iter().find(|f| f.locals.is_empty()).unwrap();
    assert!(plans(&fill, &have_sam3()).is_empty());
    let p = plan(&fill, &have_sam3());
    assert_eq!(p.state, State::NoLocalRoute);
    assert!(p.recipe.is_empty());
}

#[test]
fn each_recipe_is_its_own_setup_option_with_its_own_size_and_status() {
    let options = setup_options(&[two_models()]);
    let locals: Vec<_> = options
        .iter()
        .filter(|o| matches!(o.route, Route::Local { .. }))
        .collect();
    assert_eq!(locals.len(), 2);
    assert_eq!(locals[0].id, "local:select-subject:sam3-segment");
    assert_eq!(locals[1].id, "local:select-subject:other-segment");
    assert_eq!(
        locals[1].route,
        Route::Local {
            feature: "select-subject".into(),
            recipe: "other-segment".into(),
        }
    );
    assert_eq!(locals[1].title, "Other");
    assert_eq!(locals[1].size_bytes, Some(1_000));
    assert!(locals[0].usable);
    assert!(
        !locals[1].usable,
        "a model that cannot run yet is not offered as usable"
    );
    assert!(locals.iter().all(|o| o.unlocks == ["select-subject"]));
}
