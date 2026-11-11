//! Assigning a model to a tool and taking it back, as one shared operation on real store files.
//! The invariants are in `src/assignments.rs`: nothing is chosen as a repair, a provider's ability
//! is never removed, no binding is left stale, and a refused change leaves the file's exact bytes.

use std::sync::Arc;

use craft_ai_client::{
    Ability, AiClient, ChoiceScope, ConnectionManager, UnavailableChoice, Withdraw,
};
use craft_ai_core::{AppId, RoleId};
use craft_ai_native::secrets::MemoryStore;

#[path = "support/assignment_world.rs"]
mod assignment_world;
use assignment_world::*;

/// A listed model is saved as the suite choice, and its standing says it is chosen and usable.
#[test]
fn assigning_writes_the_binding_apps_resolve() {
    let w = world(Some(file()));
    let done = w
        .desk
        .assign_model(
            &tool("fill"),
            &one(),
            &model("reported"),
            &ChoiceScope::Suite,
            &Ability::AlreadyListed,
        )
        .unwrap();
    assert_eq!(done.replaced, None);
    let store = saved(&w);
    assert_eq!(store.defaults.len(), 1);
    assert_eq!(store.defaults[0].model.as_str(), "reported");
    let state = w
        .desk
        .model_choice_state(&tool("fill"), &one(), &model("reported"))
        .unwrap();
    assert!(state.suite_selected);
    assert_eq!(state.available, Ok(()));
}

/// A listed model without the tool's ability is refused, the file's bytes stay, and its standing
/// gives the reason.
#[test]
fn a_model_that_cannot_do_the_job_is_refused_and_the_bytes_stay() {
    let w = world(Some(file()));
    let before = bytes(&w);
    let refused = w.desk.assign_model(
        &tool("fill"),
        &one(),
        &model("bare"),
        &ChoiceScope::Suite,
        &Ability::AlreadyListed,
    );
    assert!(refused.is_err());
    assert_eq!(bytes(&w), before);
    let state = w
        .desk
        .model_choice_state(&tool("fill"), &one(), &model("bare"))
        .unwrap();
    assert!(!state.suite_selected);
    assert!(state.available.is_err(), "the reason is reported");
}

/// With no store file yet, a refused assignment leaves no file behind.
#[test]
fn a_refused_assignment_creates_no_file() {
    let w = world(None);
    let refused = w.desk.assign_model(
        &tool("fill"),
        &one(),
        &model("reported"),
        &ChoiceScope::Suite,
        &declared(),
    );
    assert!(refused.is_err());
    assert!(!w.shared.path().exists());
}

/// A model the connection does not list can be declared and chosen; withdrawing the choice and
/// ability removes the model again and leaves no binding.
#[test]
fn a_person_can_declare_an_unlisted_model_and_withdrawing_removes_it_whole() {
    let w = world(Some(file()));
    w.desk
        .assign_model(
            &tool("fill"),
            &one(),
            &model("unlisted"),
            &ChoiceScope::Suite,
            &declared(),
        )
        .unwrap();
    let store = saved(&w);
    assert_eq!(store.defaults[0].model.as_str(), "unlisted");
    assert!(store.connections[0].model(&model("unlisted")).is_some());

    let done = w
        .desk
        .withdraw_model(
            &tool("fill"),
            &one(),
            &model("unlisted"),
            &ChoiceScope::Suite,
            Withdraw::ChoiceAndDeclaredAbility,
        )
        .unwrap();
    assert!(done.choice_removed && done.ability_removed);
    let store = saved(&w);
    assert!(store.defaults.is_empty(), "no stale binding is left");
    assert!(store.connections[0].model(&model("unlisted")).is_none());
}

/// Withdrawing a choice and its declared ability from a model whose ability the provider reported
/// removes the choice only.
#[test]
fn withdrawing_never_removes_an_ability_the_provider_reported() {
    let w = world(Some(file()));
    w.desk
        .assign_model(
            &tool("fill"),
            &one(),
            &model("reported"),
            &ChoiceScope::Suite,
            &declared(),
        )
        .unwrap();
    let done = w
        .desk
        .withdraw_model(
            &tool("fill"),
            &one(),
            &model("reported"),
            &ChoiceScope::Suite,
            Withdraw::ChoiceAndDeclaredAbility,
        )
        .unwrap();
    assert!(done.choice_removed);
    assert!(!done.ability_removed);
    let store = saved(&w);
    let info = store.connections[0].model(&model("reported")).unwrap();
    assert_eq!(info.capabilities.len(), 1, "the provider's ability stays");
}

/// A declared ability another suite choice still needs is not withdrawn, and nothing is half
/// removed.
#[test]
fn withdrawing_the_ability_is_refused_while_another_tool_still_uses_it() {
    let w = world(Some(file()));
    for id in ["fill", "other"] {
        w.desk
            .assign_model(
                &tool(id),
                &one(),
                &model("bare"),
                &ChoiceScope::Suite,
                &declared(),
            )
            .unwrap();
    }
    let before = bytes(&w);
    let refused = w.desk.withdraw_model(
        &tool("fill"),
        &one(),
        &model("bare"),
        &ChoiceScope::Suite,
        Withdraw::ChoiceAndDeclaredAbility,
    );
    assert!(refused.is_err());
    assert_eq!(bytes(&w), before, "the choice was not half removed");
}

/// A declared ability an app's own choice still needs is not withdrawn from the suite scope.
#[test]
fn withdrawing_the_ability_is_refused_while_an_app_override_uses_it() {
    let w = world(Some(file()));
    w.desk
        .assign_model(
            &tool("fill"),
            &one(),
            &model("bare"),
            &app("photocraft"),
            &declared(),
        )
        .unwrap();
    let before = bytes(&w);
    let refused = w.desk.withdraw_model(
        &tool("fill"),
        &one(),
        &model("bare"),
        &ChoiceScope::Suite,
        Withdraw::ChoiceAndDeclaredAbility,
    );
    assert!(refused.is_err());
    assert_eq!(bytes(&w), before);
}

/// Withdrawing the choice of a model the tool no longer uses is refused as out of date.
#[test]
fn withdrawing_a_choice_that_has_moved_on_is_stale_and_changes_nothing() {
    let w = world(Some(file()));
    w.desk
        .assign_model(
            &tool("fill"),
            &one(),
            &model("reported"),
            &ChoiceScope::Suite,
            &Ability::AlreadyListed,
        )
        .unwrap();
    let before = bytes(&w);
    let stale = w.desk.withdraw_model(
        &tool("fill"),
        &one(),
        &model("bare"),
        &ChoiceScope::Suite,
        Withdraw::Choice,
    );
    assert!(stale.is_err());
    assert_eq!(bytes(&w), before);
}

/// After the tool moves to another model, withdrawing the old model's declared ability keeps the
/// new choice.
#[test]
fn withdrawing_an_ability_leaves_another_models_choice_for_the_tool_alone() {
    let w = world(Some(file()));
    w.desk
        .assign_model(
            &tool("fill"),
            &one(),
            &model("bare"),
            &ChoiceScope::Suite,
            &declared(),
        )
        .unwrap();
    let done = w
        .desk
        .assign_model(
            &tool("fill"),
            &one(),
            &model("reported"),
            &ChoiceScope::Suite,
            &Ability::AlreadyListed,
        )
        .unwrap();
    assert_eq!(done.replaced, Some((one(), model("bare"))));
    let gone = w
        .desk
        .withdraw_model(
            &tool("fill"),
            &one(),
            &model("bare"),
            &ChoiceScope::Suite,
            Withdraw::ChoiceAndDeclaredAbility,
        )
        .unwrap();
    assert!(!gone.choice_removed && gone.ability_removed);
    assert_eq!(saved(&w).defaults[0].model.as_str(), "reported");
}

/// A suite choice does not overwrite an app's own choice; the app is reported as choosing
/// elsewhere, and each model's standing names its scope.
#[test]
fn suite_and_app_choices_stay_apart_and_each_is_reported() {
    let w = world(Some(file()));
    w.desk
        .assign_model(
            &tool("fill"),
            &one(),
            &model("bare"),
            &app("photocraft"),
            &declared(),
        )
        .unwrap();
    let done = w
        .desk
        .assign_model(
            &tool("fill"),
            &one(),
            &model("reported"),
            &ChoiceScope::Suite,
            &Ability::AlreadyListed,
        )
        .unwrap();
    assert_eq!(done.apps_elsewhere, vec![AppId::new("photocraft").unwrap()]);
    let store = saved(&w);
    let settings = &store.apps[&AppId::new("photocraft").unwrap()];
    assert_eq!(
        settings.overrides[0].model.as_str(),
        "bare",
        "not overwritten"
    );
    let bare = w
        .desk
        .model_choice_state(&tool("fill"), &one(), &model("bare"))
        .unwrap();
    assert!(!bare.suite_selected);
    assert_eq!(bare.apps_selected, vec![AppId::new("photocraft").unwrap()]);
}

/// Withdrawing one tool's choice keeps another tool's choice on the same model and every listed
/// model.
#[test]
fn unrelated_choices_and_connections_survive() {
    let w = world(Some(file()));
    for id in ["fill", "other"] {
        w.desk
            .assign_model(
                &tool(id),
                &one(),
                &model("reported"),
                &ChoiceScope::Suite,
                &Ability::AlreadyListed,
            )
            .unwrap();
    }
    w.desk
        .withdraw_model(
            &tool("fill"),
            &one(),
            &model("reported"),
            &ChoiceScope::Suite,
            Withdraw::Choice,
        )
        .unwrap();
    let store = saved(&w);
    assert_eq!(store.defaults.len(), 1);
    assert_eq!(store.defaults[0].role.as_str(), "other");
    assert_eq!(store.connections[0].models.len(), 2);
}

/// Saving a form that drops a chosen model reports the app's choice as unavailable and keeps it.
#[test]
fn a_saved_connection_change_reports_a_choice_whose_model_went_away() {
    let w = world(Some(file()));
    w.desk
        .assign_model(
            &tool("fill"),
            &one(),
            &model("reported"),
            &app("photocraft"),
            &Ability::AlreadyListed,
        )
        .unwrap();
    let manager = ConnectionManager::new(w.shared.clone(), Arc::new(MemoryStore::new()));
    let connection = saved(&w).connections[0].clone();
    let mut draft = craft_ai_core::ConnectionDraft::from_connection(&connection);
    draft.models.retain(|m| m.id.as_str() != "reported");
    let done = manager.save(&draft).unwrap();
    assert_eq!(
        done.unavailable_choices,
        vec![UnavailableChoice {
            scope: app("photocraft"),
            tool: RoleId::new("fill").unwrap(),
            model: model("reported"),
        }]
    );
    let store = saved(&w);
    let settings = &store.apps[&AppId::new("photocraft").unwrap()];
    assert_eq!(settings.overrides.len(), 1, "the person's choice is kept");
}

/// A refused bind, choose or tool removal, and a choose that finds a choice already made, leave the
/// file's exact bytes.
#[test]
fn every_refused_store_change_keeps_the_exact_bytes() {
    let w = world(Some(file()));
    // Formatting a rewrite would change, so any write at all is visible.
    let before = bytes(&w);
    let fill = tool("fill");
    let client = AiClient::new(
        AppId::new("photocraft").unwrap(),
        w.shared.clone(),
        Arc::new(MemoryStore::new()),
    );
    assert!(client.bind(&fill, &one(), &model("bare")).is_err());
    assert!(w.desk.choose(&fill, &one(), &model("bare")).is_err());
    assert!(
        w.desk
            .remove_tool(&RoleId::new("nothing").unwrap())
            .is_err()
    );
    assert_eq!(bytes(&w), before);

    w.desk.choose(&fill, &one(), &model("reported")).unwrap();
    let before = bytes(&w);
    let again = w.desk.choose_if_unchosen(&fill, &one(), &model("reported"));
    assert!(!again.unwrap(), "a choice already exists");
    assert_eq!(bytes(&w), before, "writing nothing means writing nothing");
}
