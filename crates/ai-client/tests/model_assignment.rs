//! Assigning a model to a tool and taking it back, as one shared operation on real store files.
//! The invariants are in `src/assignments.rs`: nothing is chosen as a repair, a provider's ability
//! is never removed, no binding is left stale, and a refused change leaves the file's exact bytes.

use std::sync::Arc;

use craft_ai_client::{
    Ability, AiClient, ChoiceScope, ConnectionManager, ToolDesk, UnavailableChoice, Withdraw,
};
use craft_ai_core::{AppId, ConnectionId, ModelId, Role, RoleId, StoreFile};
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;
use serde_json::json;

struct World {
    _dir: tempfile::TempDir,
    shared: SharedStore,
    desk: ToolDesk,
}

fn world(file: Option<serde_json::Value>) -> World {
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    if let Some(file) = file {
        std::fs::write(shared.path(), file.to_string()).unwrap();
    }
    let client = AiClient::new(
        ToolDesk::app().unwrap(),
        shared.clone(),
        Arc::new(MemoryStore::new()),
    );
    let desk = ToolDesk::new(client, shared.clone(), None);
    World {
        _dir: dir,
        shared,
        desk,
    }
}

/// One connection `one`: `reported` can do images (the provider said so), `bare` can do nothing yet.
fn file() -> serde_json::Value {
    json!({
        "schema_version": 1,
        "connections": [{
            "id": "one",
            "label": "One",
            "provider": "openai",
            "endpoint": "https://one.example.com/v1",
            "locality": "remote",
            "auth": {"kind": "none"},
            "billing": "metered",
            "models": [
                {
                    "id": "reported",
                    "label": "Reported",
                    "capabilities": [{"capability": "image.generate", "accepts": {"prompt": 1}}]
                },
                {"id": "bare", "label": "Bare"}
            ]
        }]
    })
}

fn tool(id: &str) -> Role {
    Role {
        id: RoleId::new(id).unwrap(),
        label: id.to_owned(),
        capability: "image.generate".parse().unwrap(),
        inputs: vec![craft_ai_core::model::InputRole::Prompt],
    }
}

fn one() -> ConnectionId {
    ConnectionId::new("one").unwrap()
}

fn model(id: &str) -> ModelId {
    ModelId::new(id).unwrap()
}

fn app(id: &str) -> ChoiceScope {
    ChoiceScope::App(AppId::new(id).unwrap())
}

fn declared() -> Ability {
    Ability::DeclaredByPerson {
        label: "Added".to_owned(),
    }
}

fn saved(w: &World) -> StoreFile {
    w.shared.load().unwrap()
}

fn bytes(w: &World) -> Vec<u8> {
    std::fs::read(w.shared.path()).unwrap()
}

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

/// Answers a refresh with a fixed model list.
struct Lists(Vec<craft_ai_core::ModelInfo>);

impl craft_ai_core::Provider for Lists {
    fn models(&self) -> Result<Vec<craft_ai_core::ModelInfo>, craft_ai_core::AiError> {
        Ok(self.0.clone())
    }
    fn submit(
        &self,
        _: &ModelId,
        _: &craft_ai_core::Request,
    ) -> Result<craft_ai_core::JobId, craft_ai_core::AiError> {
        panic!("a refresh must not submit a job");
    }
    fn poll(
        &self,
        _: &craft_ai_core::JobId,
    ) -> Result<craft_ai_core::JobSnapshot, craft_ai_core::AiError> {
        panic!("a refresh must not poll a job");
    }
    fn cancel(&self, _: &craft_ai_core::JobId) -> Result<(), craft_ai_core::AiError> {
        Ok(())
    }
}

fn refresh(w: &World, list: Vec<craft_ai_core::ModelInfo>) -> craft_ai_client::TestReport {
    let mut factory = craft_ai_client::ProviderFactory::standard();
    factory.register("openai", move |_, _| Ok(Arc::new(Lists(list.clone()))));
    ConnectionManager::new(w.shared.clone(), Arc::new(MemoryStore::new()))
        .with_probe_factory(factory)
        .test_saved(&one(), true)
        .unwrap()
}

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
