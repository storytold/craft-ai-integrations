//! Guards around one shared assignment, on real store files: a route the adapter refuses is never
//! assigned or offered, a tool that was replaced cannot take back the new tool's choice, and saving
//! a connection form keeps what a person declared for a tool.

use std::sync::Arc;

use craft_ai_client::{
    Ability, ChoiceScope, ConnectionManager, ProviderFactory, ToolDesk, Withdraw,
};
use craft_ai_core::model::{CapabilitySpec, InputRole};
use craft_ai_core::store::MAX_STORE_BYTES;
use craft_ai_core::{ConnectionDraft, ConnectionId, ErrorKind, ModelId, ModelInfo, Role};
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;
use serde_json::json;

#[path = "support/route_fixture.rs"]
mod route_fixture;
use route_fixture::*;

/// A tool desk over `shared` with every built-in adapter.
fn desk(shared: &SharedStore) -> ToolDesk {
    ToolDesk::new(
        client(shared.clone(), ProviderFactory::standard()),
        shared.clone(),
        None,
    )
}

/// The fixture's connection.
fn one() -> ConnectionId {
    "one".parse().unwrap()
}

/// A model id.
fn model(id: &str) -> ModelId {
    id.parse().unwrap()
}

/// The person says the model can do the job.
fn declared() -> Ability {
    Ability::DeclaredByPerson {
        label: "Added".to_owned(),
    }
}

/// The store file's exact bytes.
fn bytes(shared: &SharedStore) -> Vec<u8> {
    std::fs::read(shared.path()).unwrap()
}

/// A tool `id` for `capability` that takes `inputs`.
fn tool(id: &str, capability: &str, inputs: Vec<InputRole>) -> Role {
    Role {
        id: id.parse().unwrap(),
        label: id.to_owned(),
        capability: capability.parse().unwrap(),
        inputs,
    }
}

/// A connection `one` at this computer's test address whose model `m` the provider reports for
/// image generation with a prompt. No adapter in this build says anything about `openai` routes.
fn reported_store(dir: &tempfile::TempDir) -> SharedStore {
    saved_at(
        dir,
        "openai",
        LOOPBACK,
        "m",
        "image.generate",
        json!({"prompt": 1}),
    )
}

/// A model the Omni adapter does not serve is refused with the file's bytes unchanged, whether it
/// was listed or the person declared it, and its standing says it cannot be used.
#[test]
fn assigning_a_route_the_adapter_refuses_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "artcraft-omni", "not_a_real_model");
    let desk = desk(&shared);
    let before = bytes(&shared);
    for ability in [Ability::AlreadyListed, declared()] {
        let refused = desk
            .assign_model(
                &role(),
                &one(),
                &model("other"),
                &ChoiceScope::Suite,
                &ability,
            )
            .unwrap_err();
        assert_eq!(refused.kind, ErrorKind::Unsupported, "{ability:?}");
        assert_eq!(bytes(&shared), before, "nothing was written: {ability:?}");
    }
    let state = desk
        .model_choice_state(&role(), &one(), &model("other"))
        .unwrap();
    assert!(state.available.is_err(), "a refused route is not available");
}

/// A provider with no adapter in this build is not refused: the suite choice may name a model only
/// some apps can run.
#[test]
fn a_provider_with_no_adapter_can_still_be_the_suite_choice() {
    let dir = tempfile::tempdir().unwrap();
    let shared = saved(&dir, "no-adapter-here", "m");
    let desk = desk(&shared);
    desk.assign_model(
        &role(),
        &one(),
        &model("other"),
        &ChoiceScope::Suite,
        &Ability::AlreadyListed,
    )
    .unwrap();
    let state = desk
        .model_choice_state(&role(), &one(), &model("other"))
        .unwrap();
    assert_eq!(state.available, Ok(()));
}

/// A custom tool removed and registered again for another job is a new tool. Withdrawing with the
/// old definition is refused, so the new tool keeps its choice and the file's bytes stay.
#[test]
fn withdrawing_with_a_tool_that_was_replaced_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let shared = reported_store(&dir);
    let desk = desk(&shared);
    let old = tool("brush", "image.edit", vec![InputRole::Prompt]);
    desk.register_tool(old.clone()).unwrap();
    desk.assign_model(&old, &one(), &model("m"), &ChoiceScope::Suite, &declared())
        .unwrap();
    desk.remove_tool(&old.id).unwrap();
    let new = tool("brush", "video.generate", vec![InputRole::Prompt]);
    desk.register_tool(new.clone()).unwrap();
    desk.assign_model(&new, &one(), &model("m"), &ChoiceScope::Suite, &declared())
        .unwrap();
    let before = bytes(&shared);

    let refused = desk.withdraw_model(
        &old,
        &one(),
        &model("m"),
        &ChoiceScope::Suite,
        Withdraw::Choice,
    );

    assert!(refused.is_err(), "the old definition is out of date");
    assert_eq!(bytes(&shared), before, "the new tool's choice stays");
}

/// Declares a model the connection does not list for one tool, and adds a mask to the reported
/// ability of `m` for another. Returns both tools.
fn declare_two(desk: &ToolDesk) -> [Role; 2] {
    let unlisted = tool("caption", "text.generate", vec![InputRole::Prompt]);
    let masked = tool(
        "brush",
        "image.generate",
        vec![InputRole::Prompt, InputRole::Mask],
    );
    desk.assign_model(
        &unlisted,
        &one(),
        &model("added"),
        &ChoiceScope::Suite,
        &declared(),
    )
    .unwrap();
    desk.assign_model(
        &masked,
        &one(),
        &model("m"),
        &ChoiceScope::Suite,
        &declared(),
    )
    .unwrap();
    [unlisted, masked]
}

/// Asserts every tool still resolves to its declared model.
fn assert_still_usable(desk: &ToolDesk, [unlisted, masked]: &[Role; 2]) {
    for (tool, id) in [(unlisted, "added"), (masked, "m")] {
        let state = desk.model_choice_state(tool, &one(), &model(id)).unwrap();
        assert!(state.suite_selected, "{} is still chosen", tool.id);
        assert_eq!(state.available, Ok(()), "{} still resolves", tool.id);
    }
}

/// A form opened before another app assigned models still holds the old list. Saving an unrelated
/// edit from it keeps the declared model and the declared input.
#[test]
fn saving_a_form_opened_before_an_assignment_keeps_the_declarations() {
    let dir = tempfile::tempdir().unwrap();
    let shared = reported_store(&dir);
    let desk = desk(&shared);
    let manager = ConnectionManager::new(shared.clone(), Arc::new(MemoryStore::new()));
    let mut draft = ConnectionDraft::from_connection(&shared.load().unwrap().connections[0]);
    let tools = declare_two(&desk);

    draft.label = "Renamed".to_owned();
    manager.save(&draft).unwrap();

    let store = shared.load().unwrap();
    assert_eq!(store.connections[0].label, "Renamed", "the edit is saved");
    assert_still_usable(&desk, &tools);
}

/// A test answer carries only what the server reports. Saving it keeps what a person declared on
/// top, as a remembered refresh does.
#[test]
fn saving_a_tested_form_keeps_the_declarations() {
    let dir = tempfile::tempdir().unwrap();
    let shared = reported_store(&dir);
    let desk = desk(&shared);
    let manager = ConnectionManager::new(shared.clone(), Arc::new(MemoryStore::new()));
    let tools = declare_two(&desk);
    let mut draft = ConnectionDraft::from_connection(&shared.load().unwrap().connections[0]);
    let mut m = ModelInfo::new(model("m"), "m");
    m.capabilities.push(
        CapabilitySpec::new("image.generate".parse().unwrap()).accepting(InputRole::Prompt, 1),
    );
    assert!(draft.set_models(LOOPBACK, vec![m]));

    manager.save(&draft).unwrap();

    assert_still_usable(&desk, &tools);
}

/// A write that fails after the change was made in memory (here the file would grow past the
/// store's size limit) leaves the exact bytes: no model, ability or choice lands on its own.
#[test]
fn a_failed_write_leaves_no_part_of_an_assignment() {
    let dir = tempfile::tempdir().unwrap();
    let shared = reported_store(&dir);
    // How long the file is once the store has rewritten it, with an empty field it does not know.
    let mut file: serde_json::Value = serde_json::from_slice(&bytes(&shared)).unwrap();
    file["padding"] = json!("");
    std::fs::write(shared.path(), file.to_string()).unwrap();
    shared.update(|_| ()).unwrap();
    let written = bytes(&shared).len();
    // Room for the store's own rewrite, but not for a declared model, its ability and the choice.
    file["padding"] = json!("x".repeat(MAX_STORE_BYTES - written - 40));
    std::fs::write(shared.path(), file.to_string()).unwrap();
    shared.update(|_| ()).unwrap();
    let before = bytes(&shared);

    let failed = desk(&shared).assign_model(
        &tool(
            "brush",
            "image.generate",
            vec![InputRole::Prompt, InputRole::Mask],
        ),
        &one(),
        &model("added"),
        &ChoiceScope::Suite,
        &declared(),
    );

    let error = failed.unwrap_err();
    assert!(
        error.message.contains("the limit is"),
        "the write failed for its size: {}",
        error.message
    );
    assert_eq!(bytes(&shared), before, "nothing landed");
}

/// A form opened before the service listed a model does not drop that model on save: another app
/// declared an ability on it and chose it in the meantime, and the choice still resolves.
#[test]
fn saving_a_form_opened_before_a_model_was_listed_keeps_it() {
    let dir = tempfile::tempdir().unwrap();
    let shared = reported_store(&dir);
    let desk = desk(&shared);
    let manager = ConnectionManager::new(shared.clone(), Arc::new(MemoryStore::new()));
    let mut draft = ConnectionDraft::from_connection(&shared.load().unwrap().connections[0]);
    // A refresh elsewhere lists a new model, then another app gives it a job and chooses it.
    shared
        .update(|store| {
            store.connections[0]
                .models
                .push(ModelInfo::new(model("late"), "Late"))
        })
        .unwrap();
    let caption = tool("caption", "text.generate", vec![InputRole::Prompt]);
    desk.assign_model(
        &caption,
        &one(),
        &model("late"),
        &ChoiceScope::Suite,
        &declared(),
    )
    .unwrap();

    draft.label = "Renamed".to_owned();
    manager.save(&draft).unwrap();

    let state = desk
        .model_choice_state(&caption, &one(), &model("late"))
        .unwrap();
    assert!(state.suite_selected);
    assert_eq!(
        state.available,
        Ok(()),
        "the later model and its ability stay"
    );
}
