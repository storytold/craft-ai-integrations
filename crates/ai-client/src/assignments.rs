//! One shared operation for "this model is used for this tool".
//!
//! A selected tool has two facts in the store: the binding, and the model's ability to do the job.
//! Writing them separately let a host withdraw an ability while the binding stayed, which left a
//! selected tool with no eligible model. Here both change under one store lock, so a person's choice
//! either lands whole or leaves the file's bytes as they were.
//!
//! Invariants:
//! - The caller states its intent. A model is never chosen as a side effect of a repair.
//! - An ability a provider reported is never removed. An ability a person declared carries a tag
//!   ([`declared_tag`]), because comparing its shape cannot tell the two apart.
//! - Withdrawing refuses while another binding still needs the ability, rather than leaving that
//!   binding stale or silently deleting someone's choice.
//! - A suite choice never touches an app's own override, and an app choice never touches the suite
//!   default. The other layer is reported so a host can show it.

use craft_ai_core::model::{CapabilitySpec, ModelInfo};
use craft_ai_core::role::resolve;
use craft_ai_core::{
    AiError, AppId, Binding, CapabilityId, Connection, ConnectionId, ModelId, Role, RoleId,
    StoreFile,
};
use craft_ai_features::builtin;

use crate::tools::ToolDesk;

/// Marks a model the connection did not list, added only because a person assigned it to a tool.
const DECLARED_MODEL: &str = "declared-model";

/// Marks an ability a person declared, as opposed to one the provider reported.
fn declared_tag(capability: &CapabilityId) -> String {
    format!("declared:{capability}")
}

/// Which choice an operation acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChoiceScope {
    /// The launcher's default, which every app without its own choice inherits.
    Suite,
    /// One app's own choice.
    App(AppId),
}

/// How a model comes to be able to do a tool's job.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ability {
    /// The model must already be able to. Refused otherwise.
    AlreadyListed,
    /// The person says it can. The model is added under `label` if the connection does not list
    /// it, and gains the tool's capability and inputs. Nothing is tested or billed; a sample run
    /// is what proves it works.
    DeclaredByPerson { label: String },
}

/// What assigning did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assigned {
    /// The choice this one replaced in the same scope, if it was a different one.
    pub replaced: Option<(ConnectionId, ModelId)>,
    /// Apps with their own different choice for this tool, which were left alone.
    pub apps_elsewhere: Vec<AppId>,
}

/// What to take back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Withdraw {
    /// Only the choice. The model keeps every ability it has.
    Choice,
    /// The choice, if it is this model, and the ability if a person declared it.
    ChoiceAndDeclaredAbility,
}

/// What withdrawing did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Withdrawn {
    pub choice_removed: bool,
    pub ability_removed: bool,
}

/// One model's standing for one tool, read from the shared store rather than a host's own notes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolChoiceState {
    /// It is the suite's choice for this tool.
    pub suite_selected: bool,
    /// Apps whose own choice for this tool is this model.
    pub apps_selected: Vec<AppId>,
    /// Apps whose own choice for this tool is a different model.
    pub apps_elsewhere: Vec<AppId>,
    /// Whether the model can be used for the tool, or why not.
    pub available: Result<(), String>,
}

impl ToolDesk {
    /// Makes this model the choice for `role` in `scope`, declaring its ability first if asked.
    /// Refused, with the file's bytes unchanged, if the result would not resolve.
    pub fn assign_model(
        &self,
        role: &Role,
        connection: &ConnectionId,
        model: &ModelId,
        scope: &ChoiceScope,
        ability: &Ability,
    ) -> Result<Assigned, AiError> {
        let binding = Binding {
            role: role.id.clone(),
            connection: connection.clone(),
            model: model.clone(),
        };
        self.shared.try_update(|store| {
            store
                .check_tool(role)
                .map_err(|e| AiError::invalid(e.to_string()))?;
            if let Ability::DeclaredByPerson { label } = ability {
                declare(store, role, connection, model, label)?;
            }
            resolve(role, &[std::slice::from_ref(&binding)], &store.connections)
                .map_err(|e| AiError::invalid(e.to_string()))?;
            let layer = layer_mut(store, scope);
            let replaced = layer
                .iter()
                .find(|b| b.role == role.id && (&b.connection, &b.model) != (connection, model))
                .map(|b| (b.connection.clone(), b.model.clone()));
            layer.retain(|b| b.role != role.id);
            layer.push(binding.clone());
            Ok::<_, AiError>(Assigned {
                replaced,
                apps_elsewhere: apps_on(store, &role.id, |b| {
                    (&b.connection, &b.model) != (connection, model)
                }),
            })
        })?
    }

    /// Takes back this model's assignment to `role` in `scope`. Another model is never chosen in
    /// its place.
    ///
    /// [`Withdraw::Choice`] is refused when the tool now uses another model, because the person
    /// was looking at something out of date. [`Withdraw::ChoiceAndDeclaredAbility`] leaves another
    /// model's choice alone and still removes the ability, unless a binding in any scope needs it.
    pub fn withdraw_model(
        &self,
        role: &Role,
        connection: &ConnectionId,
        model: &ModelId,
        scope: &ChoiceScope,
        what: Withdraw,
    ) -> Result<Withdrawn, AiError> {
        self.shared.try_update(|store| {
            // An app with no settings yet has no choice to withdraw, and gets no empty entry.
            let layer = match scope {
                ChoiceScope::Suite => Some(&mut store.defaults),
                ChoiceScope::App(app) => store.apps.get_mut(app).map(|s| &mut s.overrides),
            };
            let selected = layer
                .as_deref()
                .and_then(|layer| layer.iter().find(|b| b.role == role.id))
                .map(|b| (&b.connection, &b.model) == (connection, model));
            if what == Withdraw::Choice && selected != Some(true) {
                return Err(AiError::invalid(
                    "that tool no longer uses this model; nothing was changed",
                ));
            }
            let choice_removed = selected == Some(true);
            if let (true, Some(layer)) = (choice_removed, layer) {
                layer.retain(|b| b.role != role.id);
            }
            let ability_removed = what == Withdraw::ChoiceAndDeclaredAbility
                && forget_declared(store, role, connection, model)?;
            Ok(Withdrawn {
                choice_removed,
                ability_removed,
            })
        })?
    }

    /// Where this model stands for `role`. Reads only.
    pub fn model_choice_state(
        &self,
        role: &Role,
        connection: &ConnectionId,
        model: &ModelId,
    ) -> Result<ToolChoiceState, AiError> {
        let store = self.shared.load()?;
        let here = |b: &Binding| (&b.connection, &b.model) == (connection, model);
        let binding = Binding {
            role: role.id.clone(),
            connection: connection.clone(),
            model: model.clone(),
        };
        let available = store
            .check_tool(role)
            .map_err(|e| e.to_string())
            .and_then(|()| {
                resolve(role, &[std::slice::from_ref(&binding)], &store.connections)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            });
        Ok(ToolChoiceState {
            suite_selected: store.defaults.iter().any(|b| b.role == role.id && here(b)),
            apps_selected: apps_on(&store, &role.id, here),
            apps_elsewhere: apps_on(&store, &role.id, |b| !here(b)),
            available,
        })
    }
}

fn layer_mut<'a>(store: &'a mut StoreFile, scope: &ChoiceScope) -> &'a mut Vec<Binding> {
    match scope {
        ChoiceScope::Suite => &mut store.defaults,
        ChoiceScope::App(app) => &mut store.apps.entry(app.clone()).or_default().overrides,
    }
}

/// Apps whose own choice for `role` satisfies `test`.
fn apps_on(store: &StoreFile, role: &RoleId, test: impl Fn(&Binding) -> bool) -> Vec<AppId> {
    store
        .apps
        .iter()
        .filter(|(_, settings)| {
            settings
                .overrides
                .iter()
                .any(|b| &b.role == role && test(b))
        })
        .map(|(app, _)| app.clone())
        .collect()
}

fn connection_mut<'a>(
    store: &'a mut StoreFile,
    id: &ConnectionId,
) -> Result<&'a mut Connection, AiError> {
    store
        .connections
        .iter_mut()
        .find(|c| &c.id == id)
        .ok_or_else(|| AiError::invalid(format!("connection {id} no longer exists")))
}

/// Writes the ability onto the model. An ability the provider reported is never narrowed, and
/// inputs added to it are not tagged, so withdrawing can never take the provider's ability away.
fn declare(
    store: &mut StoreFile,
    role: &Role,
    connection: &ConnectionId,
    model: &ModelId,
    label: &str,
) -> Result<(), AiError> {
    let saved = connection_mut(store, connection)?;
    if saved.model(model).is_none() {
        let mut added = ModelInfo::new(model.clone(), label);
        added.tags.push(DECLARED_MODEL.to_owned());
        saved.models.push(added);
    }
    let info = saved
        .models
        .iter_mut()
        .find(|m| &m.id == model)
        .ok_or_else(|| AiError::invalid("that model is not listed for this connection"))?;
    match info
        .capabilities
        .iter_mut()
        .find(|spec| spec.capability == role.capability)
    {
        Some(spec) => {
            for input in &role.inputs {
                spec.accepts.entry(input.clone()).or_insert(1);
            }
        }
        None => {
            let mut spec = CapabilitySpec::new(role.capability.clone());
            for input in &role.inputs {
                spec = spec.accepting(input.clone(), 1);
            }
            info.capabilities.push(spec);
            info.tags.push(declared_tag(&role.capability));
        }
    }
    Ok(())
}

/// Removes the ability if a person declared it. Refused while a binding still needs it.
fn forget_declared(
    store: &mut StoreFile,
    role: &Role,
    connection: &ConnectionId,
    model: &ModelId,
) -> Result<bool, AiError> {
    let tag = declared_tag(&role.capability);
    let declared = store
        .connections
        .iter()
        .find(|c| &c.id == connection)
        .and_then(|c| c.model(model))
        .is_some_and(|m| m.tags.contains(&tag));
    if !declared {
        return Ok(false);
    }
    if let Some(user) = binding_needing(store, role, connection, model) {
        return Err(AiError::invalid(format!(
            "another choice still uses this model ({user}); change that choice first"
        )));
    }
    let saved = connection_mut(store, connection)?;
    let Some(index) = saved.models.iter().position(|m| &m.id == model) else {
        return Ok(false);
    };
    let info = &mut saved.models[index];
    info.capabilities
        .retain(|spec| spec.capability != role.capability);
    info.tags.retain(|t| t != &tag);
    if info.capabilities.is_empty() && info.tags.iter().any(|t| t == DECLARED_MODEL) {
        saved.models.remove(index);
    }
    Ok(true)
}

/// A binding on this model that needs `role`'s capability, in any scope. A binding for a tool this
/// build does not know counts as needing it, since the ability might be what it relies on.
fn binding_needing(
    store: &StoreFile,
    role: &Role,
    connection: &ConnectionId,
    model: &ModelId,
) -> Option<RoleId> {
    let features = builtin();
    let needs = |tool: &RoleId| {
        let known = store
            .custom_tools
            .iter()
            .find(|t| &t.id == tool)
            .map(|t| t.capability.clone())
            .or_else(|| {
                features
                    .iter()
                    .find(|f| f.id == tool.as_str())
                    .and_then(|f| f.role())
                    .map(|r| r.capability)
            });
        known.is_none_or(|capability| capability == role.capability)
    };
    store
        .defaults
        .iter()
        .chain(store.apps.values().flat_map(|a| a.overrides.iter()))
        .find(|b| (&b.connection, &b.model) == (connection, model) && needs(&b.role))
        .map(|b| b.role.clone())
}

/// A saved list after a refresh: what the server reported, plus what a person declared on top. A
/// declared ability the server now reports itself stops being marked as declared.
pub(crate) fn refreshed_models(old: &[ModelInfo], reported: Vec<ModelInfo>) -> Vec<ModelInfo> {
    let mut models = reported;
    for before in old {
        match models.iter_mut().find(|m| m.id == before.id) {
            Some(now) => {
                for spec in &before.capabilities {
                    let tag = declared_tag(&spec.capability);
                    if before.tags.contains(&tag) && now.spec_for(&spec.capability).is_none() {
                        now.capabilities.push(spec.clone());
                        now.tags.push(tag);
                    }
                }
            }
            None if before.tags.iter().any(|t| t == DECLARED_MODEL) => models.push(before.clone()),
            None => {}
        }
    }
    models
}

/// A person's choice whose model the connection no longer offers. The choice is kept, so it works
/// again if the model comes back, and it is reported so a host can say so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnavailableChoice {
    pub scope: ChoiceScope,
    pub tool: RoleId,
    pub model: ModelId,
}

/// Every choice on `connection` whose model it no longer lists.
pub(crate) fn unavailable_choices(
    store: &StoreFile,
    connection: &ConnectionId,
) -> Vec<UnavailableChoice> {
    let Some(saved) = store.connections.iter().find(|c| &c.id == connection) else {
        return Vec::new();
    };
    let gone = |b: &&Binding| &b.connection == connection && saved.model(&b.model).is_none();
    let mut out: Vec<UnavailableChoice> = store
        .defaults
        .iter()
        .filter(gone)
        .map(|b| UnavailableChoice {
            scope: ChoiceScope::Suite,
            tool: b.role.clone(),
            model: b.model.clone(),
        })
        .collect();
    for (app, settings) in &store.apps {
        out.extend(
            settings
                .overrides
                .iter()
                .filter(gone)
                .map(|b| UnavailableChoice {
                    scope: ChoiceScope::App(app.clone()),
                    tool: b.role.clone(),
                    model: b.model.clone(),
                }),
        );
    }
    out
}
