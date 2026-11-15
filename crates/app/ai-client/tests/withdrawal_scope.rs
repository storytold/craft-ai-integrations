//! Withdrawing a declaration that a form opened earlier still holds, withdrawing only what no other
//! choice needs, and saying which choice blocks a withdrawal. Also: changes to an unknown connection
//! or an absent choice write nothing.

use std::sync::Arc;

use craft_ai_client::{Ability, AiClient, ChoiceScope, ConnectionManager, Withdraw};
use craft_ai_core::model::InputRole;
use craft_ai_core::{AppId, ConnectionDraft, ConnectionId, Role, RoleId};
use craft_ai_native::secrets::MemoryStore;

#[path = "support/assignment_world.rs"]
mod assignment_world;
use assignment_world::*;

/// A tool for image generation that takes a prompt and a mask.
fn brush(id: &str) -> Role {
    Role {
        id: RoleId::new(id).unwrap(),
        label: id.to_owned(),
        capability: "image.generate".parse().unwrap(),
        inputs: vec![InputRole::Prompt, InputRole::Mask],
    }
}

fn manager(w: &World) -> ConnectionManager {
    ConnectionManager::new(w.shared.clone(), Arc::new(MemoryStore::new()))
}

/// The reviewer's repro: a form opened before a withdrawal, saved with only a rename, must not
/// bring back the withdrawn model or the withdrawn ability.
#[test]
fn a_form_opened_before_a_withdrawal_does_not_bring_it_back() {
    let w = world(Some(file()));
    let fill = tool("fill");
    for id in ["bare", "ghost"] {
        w.desk
            .assign_model(&fill, &one(), &model(id), &app("photocraft"), &declared())
            .unwrap();
    }
    let mut draft = ConnectionDraft::from_connection(&saved(&w).connections[0]);
    for id in ["bare", "ghost"] {
        // The second assignment replaced the first, so `bare` has only its ability left.
        w.desk
            .withdraw_model(
                &fill,
                &one(),
                &model(id),
                &app("photocraft"),
                Withdraw::ChoiceAndDeclaredAbility,
            )
            .unwrap();
    }

    draft.label = "Renamed".to_owned();
    manager(&w).save(&draft).unwrap();

    let store = saved(&w);
    let c = &store.connections[0];
    assert_eq!(c.label, "Renamed", "the edit is saved");
    assert!(
        c.model(&model("ghost")).is_none(),
        "the withdrawn model stays gone"
    );
    let bare = c.model(&model("bare")).unwrap();
    assert!(
        bare.capabilities.is_empty(),
        "the withdrawn ability stays gone"
    );
    assert!(bare.tags.is_empty(), "no declared tag comes back");
}

/// A declared input withdrawn after the form opened stays withdrawn, and the provider's own
/// ability is left as it was.
#[test]
fn a_form_opened_before_a_withdrawal_does_not_bring_back_an_input() {
    let w = world(Some(file()));
    let tool = brush("brush");
    w.desk
        .assign_model(
            &tool,
            &one(),
            &model("reported"),
            &ChoiceScope::Suite,
            &declared(),
        )
        .unwrap();
    let draft = ConnectionDraft::from_connection(&saved(&w).connections[0]);
    w.desk
        .withdraw_model(
            &tool,
            &one(),
            &model("reported"),
            &ChoiceScope::Suite,
            Withdraw::ChoiceAndDeclaredAbility,
        )
        .unwrap();

    manager(&w).save(&draft).unwrap();

    let store = saved(&w);
    let reported = store.connections[0].model(&model("reported")).unwrap();
    let spec = reported
        .spec_for(&"image.generate".parse().unwrap())
        .unwrap();
    assert!(
        spec.accepts.contains_key(&InputRole::Prompt),
        "the reported input stays"
    );
    assert!(
        !spec.accepts.contains_key(&InputRole::Mask),
        "the withdrawn input stays gone"
    );
    assert!(reported.tags.is_empty());
}

/// A choice whose tool does not take the declared input does not block withdrawing that input.
/// One that does take it still does.
#[test]
fn only_a_choice_that_takes_the_declared_input_blocks_withdrawing_it() {
    // Each tool is saved, so its inputs are known. A tool this build does not know always blocks.
    let mut store = file();
    store["custom_tools"] = serde_json::json!([
        serde_json::to_value(brush("brush")).unwrap(),
        serde_json::to_value(tool("fill")).unwrap(),
        serde_json::to_value(brush("masked")).unwrap(),
    ]);
    let w = world(Some(store));
    let paint = brush("brush");
    w.desk
        .assign_model(
            &paint,
            &one(),
            &model("reported"),
            &app("photocraft"),
            &declared(),
        )
        .unwrap();
    let fill = tool("fill");
    w.desk
        .assign_model(
            &fill,
            &one(),
            &model("reported"),
            &ChoiceScope::Suite,
            &Ability::AlreadyListed,
        )
        .unwrap();
    let masked = brush("masked");
    w.desk
        .assign_model(
            &masked,
            &one(),
            &model("reported"),
            &app("lightcraft"),
            &declared(),
        )
        .unwrap();

    let refused = w
        .desk
        .withdraw_model(
            &paint,
            &one(),
            &model("reported"),
            &app("photocraft"),
            Withdraw::ChoiceAndDeclaredAbility,
        )
        .unwrap_err();
    assert!(
        refused.message.contains("masked") && refused.message.contains("lightcraft"),
        "the tool that needs the mask, and where it is chosen, are named: {}",
        refused.message
    );

    w.desk
        .withdraw_model(
            &masked,
            &one(),
            &model("reported"),
            &app("lightcraft"),
            Withdraw::Choice,
        )
        .unwrap();
    let done = w
        .desk
        .withdraw_model(
            &paint,
            &one(),
            &model("reported"),
            &app("photocraft"),
            Withdraw::ChoiceAndDeclaredAbility,
        )
        .unwrap();
    assert!(
        done.choice_removed && done.ability_removed,
        "fill does not need the mask"
    );
    let state = w
        .desk
        .model_choice_state(&fill, &one(), &model("reported"))
        .unwrap();
    assert_eq!(state.available, Ok(()), "fill still resolves");
}

/// When an app's own choice of the same tool blocks a suite withdrawal, the refusal says it is that
/// app's choice, so the person knows what to change.
#[test]
fn a_refusal_names_the_scope_that_blocks_it() {
    let w = world(Some(file()));
    let fill = tool("fill");
    w.desk
        .assign_model(
            &fill,
            &one(),
            &model("bare"),
            &ChoiceScope::Suite,
            &declared(),
        )
        .unwrap();
    w.desk
        .assign_model(
            &fill,
            &one(),
            &model("bare"),
            &app("photocraft"),
            &Ability::AlreadyListed,
        )
        .unwrap();
    let before = bytes(&w);

    let refused = w
        .desk
        .withdraw_model(
            &fill,
            &one(),
            &model("bare"),
            &ChoiceScope::Suite,
            Withdraw::ChoiceAndDeclaredAbility,
        )
        .unwrap_err();

    assert!(
        refused.message.contains("photocraft's own choice"),
        "{}",
        refused.message
    );
    assert_eq!(bytes(&w), before, "a refusal writes nothing");
}

/// Enabling, disabling or deleting a connection that does not exist, and clearing a choice an app
/// never made, write nothing: no file is created and no bytes change.
#[test]
fn changes_with_nothing_to_change_write_nothing() {
    let empty = world(None);
    let ghost = ConnectionId::new("ghost").unwrap();
    assert!(manager(&empty).set_enabled(&ghost, false).is_err());
    let client = AiClient::new(
        AppId::new("photocraft").unwrap(),
        empty.shared.clone(),
        Arc::new(MemoryStore::new()),
    );
    client.clear_binding(&RoleId::new("fill").unwrap()).unwrap();
    assert!(!empty.shared.path().exists(), "no file is created");

    let w = world(Some(file()));
    let before = bytes(&w);
    assert!(manager(&w).set_enabled(&ghost, true).is_err());
    assert!(manager(&w).delete(&ghost).is_err());
    let client = AiClient::new(
        AppId::new("photocraft").unwrap(),
        w.shared.clone(),
        Arc::new(MemoryStore::new()),
    );
    client.clear_binding(&RoleId::new("fill").unwrap()).unwrap();
    assert_eq!(bytes(&w), before, "the exact bytes are kept");
}
