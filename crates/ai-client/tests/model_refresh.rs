//! A remembered refresh replaces a connection's model list with what the server reports. What a
//! person declared on top stays, and a choice the new list can no longer serve is reported and kept.

use std::sync::Arc;

use craft_ai_client::{Ability, ChoiceScope, ConnectionManager, UnavailableChoice, Withdraw};
use craft_ai_core::{ModelId, RoleId};
use craft_ai_native::secrets::MemoryStore;

#[path = "support/assignment_world.rs"]
mod assignment_world;
use assignment_world::*;

/// Answers a refresh with a fixed model list.
struct Lists(Vec<craft_ai_core::ModelInfo>);

impl craft_ai_core::Provider for Lists {
    /// The fixed list.
    fn models(&self) -> Result<Vec<craft_ai_core::ModelInfo>, craft_ai_core::AiError> {
        Ok(self.0.clone())
    }
    /// Fails the test: a refresh never submits.
    fn submit(
        &self,
        _: &ModelId,
        _: &craft_ai_core::Request,
    ) -> Result<craft_ai_core::JobId, craft_ai_core::AiError> {
        panic!("a refresh must not submit a job");
    }
    /// Fails the test: a refresh never polls.
    fn poll(
        &self,
        _: &craft_ai_core::JobId,
    ) -> Result<craft_ai_core::JobSnapshot, craft_ai_core::AiError> {
        panic!("a refresh must not poll a job");
    }
    /// Accepts a cancel, which sends nothing.
    fn cancel(&self, _: &craft_ai_core::JobId) -> Result<(), craft_ai_core::AiError> {
        Ok(())
    }
}

/// Tests the saved connection `one` with `remember`, the server answering `list`.
fn refresh(w: &World, list: Vec<craft_ai_core::ModelInfo>) -> craft_ai_client::TestReport {
    let mut factory = craft_ai_client::ProviderFactory::standard();
    factory.register("openai", move |_, _| Ok(Arc::new(Lists(list.clone()))));
    ConnectionManager::new(w.shared.clone(), Arc::new(MemoryStore::new()))
        .with_probe_factory(factory)
        .test_saved(&one(), true)
        .unwrap()
}

/// A refresh keeps a declared ability on a listed model and a declared model the server does not
/// list, while a provider-listed model the server dropped goes.
#[test]
fn a_refresh_keeps_declared_abilities_and_models() {
    let w = world(Some(file()));
    for (id, name) in [("fill", "bare"), ("other", "unlisted")] {
        w.desk
            .assign_model(
                &tool(id),
                &one(),
                &model(name),
                &ChoiceScope::Suite,
                &declared(),
            )
            .unwrap();
    }
    let reported = vec![craft_ai_core::ModelInfo::new(model("bare"), "Bare")];
    let report = refresh(&w, reported);
    assert_eq!(report.unavailable_choices, Vec::new());
    let store = saved(&w);
    let bare = store.connections[0].model(&model("bare")).unwrap();
    assert_eq!(bare.capabilities.len(), 1, "the person's declaration stays");
    assert!(store.connections[0].model(&model("unlisted")).is_some());
    assert!(
        store.connections[0].model(&model("reported")).is_none(),
        "a model the server dropped goes"
    );
}

/// An input a person added to an ability the server reports comes back after a refresh, and
/// withdrawing takes back only that input.
#[test]
fn a_refresh_keeps_an_input_a_person_added_to_a_reported_ability() {
    let w = world(Some(file()));
    let mut wider = tool("fill");
    wider.inputs.push(craft_ai_core::model::InputRole::Mask);
    w.desk
        .assign_model(
            &wider,
            &one(),
            &model("reported"),
            &ChoiceScope::Suite,
            &declared(),
        )
        .unwrap();
    let mut reported = craft_ai_core::ModelInfo::new(model("reported"), "Reported");
    reported.capabilities.push(
        craft_ai_core::model::CapabilitySpec::new("image.generate".parse().unwrap())
            .accepting(craft_ai_core::model::InputRole::Prompt, 1),
    );
    refresh(&w, vec![reported]);
    let state = w
        .desk
        .model_choice_state(&wider, &one(), &model("reported"))
        .unwrap();
    assert_eq!(state.available, Ok(()), "the added input came back");

    let done = w
        .desk
        .withdraw_model(
            &wider,
            &one(),
            &model("reported"),
            &ChoiceScope::Suite,
            Withdraw::ChoiceAndDeclaredAbility,
        )
        .unwrap();
    assert!(done.ability_removed);
    let store = saved(&w);
    let spec = store.connections[0]
        .model(&model("reported"))
        .and_then(|m| m.spec_for(&"image.generate".parse().unwrap()))
        .cloned()
        .unwrap();
    assert!(
        spec.accepts
            .contains_key(&craft_ai_core::model::InputRole::Prompt)
    );
    assert!(
        !spec
            .accepts
            .contains_key(&craft_ai_core::model::InputRole::Mask)
    );
}

/// A refresh that drops a chosen model reports the choice and keeps it, choosing nothing else.
#[test]
fn a_refresh_reports_a_choice_it_could_not_keep_and_leaves_the_binding() {
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
    let report = refresh(
        &w,
        vec![craft_ai_core::ModelInfo::new(model("bare"), "Bare")],
    );
    assert_eq!(
        report.unavailable_choices,
        vec![UnavailableChoice {
            scope: ChoiceScope::Suite,
            tool: RoleId::new("fill").unwrap(),
            model: model("reported"),
        }]
    );
    assert_eq!(saved(&w).defaults.len(), 1, "no other model was chosen");
}

/// A declared ability the server starts reporting becomes the server's: withdrawing leaves it, with
/// the server's limits.
#[test]
fn an_ability_the_server_now_reports_stops_counting_as_declared() {
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
    let spec = craft_ai_core::model::CapabilitySpec::new("image.generate".parse().unwrap())
        .accepting(craft_ai_core::model::InputRole::Prompt, 4);
    let now = craft_ai_core::ModelInfo::new(model("bare"), "Bare").with(spec);
    refresh(&w, vec![now]);
    let done = w
        .desk
        .withdraw_model(
            &tool("fill"),
            &one(),
            &model("bare"),
            &ChoiceScope::Suite,
            Withdraw::ChoiceAndDeclaredAbility,
        )
        .unwrap();
    assert!(done.choice_removed);
    assert!(!done.ability_removed, "it is the server's now");
    let store = saved(&w);
    let bare = store.connections[0].model(&model("bare")).unwrap();
    assert_eq!(bare.capabilities[0].accepts.values().next(), Some(&4));
}

/// A model the server still lists, but now without the ability or the input the tool needs, is
/// reported like one that went away, and the choice is kept.
#[test]
fn a_refresh_reports_a_choice_whose_model_stays_listed_but_lost_the_ability() {
    for spec in [
        None,
        Some(
            craft_ai_core::model::CapabilitySpec::new("image.generate".parse().unwrap())
                .accepting(craft_ai_core::model::InputRole::Mask, 1),
        ),
    ] {
        let w = world(Some(file()));
        // Saved as a custom tool, so the report knows what the choice needs.
        w.desk.register_tool(tool("fill")).unwrap();
        w.desk
            .assign_model(
                &tool("fill"),
                &one(),
                &model("reported"),
                &ChoiceScope::Suite,
                &Ability::AlreadyListed,
            )
            .unwrap();
        let mut listed = craft_ai_core::ModelInfo::new(model("reported"), "Reported");
        listed.capabilities.extend(spec);
        let report = refresh(&w, vec![listed]);
        assert_eq!(
            report.unavailable_choices,
            vec![UnavailableChoice {
                scope: ChoiceScope::Suite,
                tool: RoleId::new("fill").unwrap(),
                model: model("reported"),
            }]
        );
        assert_eq!(saved(&w).defaults.len(), 1, "the choice is kept");
    }
}

/// An input a person added survives a refresh that drops the whole ability, and comes back when a
/// later refresh reports the ability again without it.
#[test]
fn a_declared_input_survives_two_refreshes_that_drop_and_restore_the_ability() {
    let w = world(Some(file()));
    let mut wider = tool("fill");
    wider.inputs.push(craft_ai_core::model::InputRole::Mask);
    w.desk
        .assign_model(
            &wider,
            &one(),
            &model("reported"),
            &ChoiceScope::Suite,
            &declared(),
        )
        .unwrap();

    refresh(
        &w,
        vec![craft_ai_core::ModelInfo::new(model("reported"), "Reported")],
    );
    let mut restored = craft_ai_core::ModelInfo::new(model("reported"), "Reported");
    restored.capabilities.push(
        craft_ai_core::model::CapabilitySpec::new("image.generate".parse().unwrap())
            .accepting(craft_ai_core::model::InputRole::Prompt, 1),
    );
    refresh(&w, vec![restored]);

    let state = w
        .desk
        .model_choice_state(&wider, &one(), &model("reported"))
        .unwrap();
    assert_eq!(state.available, Ok(()), "the declared input came back");
}
