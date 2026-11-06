//! Reconciling a connection's model list with what a person declared on it.
//!
//! A remembered refresh and a saved connection form both replace the list. What a person declared
//! for a tool (a model, an ability, an input) is put back on top of the new list, and every choice
//! the new list can no longer serve is reported, never removed.

use craft_ai_core::model::ModelInfo;
use craft_ai_core::role::{ResolveError, resolve};
use craft_ai_core::{Binding, ConnectionId, ModelId, RoleId, StoreFile};
use craft_ai_features::builtin;

use super::{ChoiceScope, DECLARED_MODEL, declared_input_tag, declared_tag, known_tool};

/// A saved list after a refresh or a saved form: the new list, plus what a person declared on top of
/// the old one. A declared ability or input the new list already has stops being marked as
/// declared, since the provider now reports it.
pub(crate) fn refreshed_models(old: &[ModelInfo], reported: Vec<ModelInfo>) -> Vec<ModelInfo> {
    let mut models = reported;
    for before in old {
        match models.iter_mut().find(|m| m.id == before.id) {
            Some(now) => carry_declared(before, now),
            None if before.tags.iter().any(|t| t == DECLARED_MODEL) => models.push(before.clone()),
            None => {}
        }
    }
    models
}

/// Puts what a person declared on `before` back onto `now`. A whole declared ability the provider
/// does not report comes back as it was. Where the provider reports the ability, only the inputs a
/// person declared and the provider lacks are added, each tagged so it can be taken back later.
fn carry_declared(before: &ModelInfo, now: &mut ModelInfo) {
    for spec in &before.capabilities {
        let tag = declared_tag(&spec.capability);
        let whole = before.tags.contains(&tag);
        if now.tags.contains(&tag) {
            continue;
        }
        match now
            .capabilities
            .iter_mut()
            .find(|s| s.capability == spec.capability)
        {
            None if whole => {
                now.capabilities.push(spec.clone());
                now.tags.push(tag);
            }
            None => {}
            Some(reported) => {
                for (input, most) in &spec.accepts {
                    let input_tag = declared_input_tag(&spec.capability, input);
                    let declared = whole || before.tags.contains(&input_tag);
                    if declared && !reported.accepts.contains_key(input) {
                        reported.accepts.insert(input.clone(), *most);
                        if !now.tags.contains(&input_tag) {
                            now.tags.push(input_tag);
                        }
                    }
                }
            }
        }
    }
}

/// A person's choice the connection can no longer serve: its model is gone, or still listed but no
/// longer able to do the tool's job. The choice is kept, so it works again if the model or its
/// ability comes back, and it is reported so a host can say so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnavailableChoice {
    pub scope: ChoiceScope,
    pub tool: RoleId,
    pub model: ModelId,
}

/// Every choice on `connection` whose model it no longer lists, or lists without the ability or
/// inputs the tool needs. A choice for a tool this build does not know is judged by its model alone.
pub(crate) fn unavailable_choices(
    store: &StoreFile,
    connection: &ConnectionId,
) -> Vec<UnavailableChoice> {
    let Some(saved) = store.connections.iter().find(|c| &c.id == connection) else {
        return Vec::new();
    };
    let features = builtin();
    let gone = |b: &&Binding| {
        if &b.connection != connection {
            return false;
        }
        if saved.model(&b.model).is_none() {
            return true;
        }
        known_tool(store, &features, &b.role).is_some_and(|tool| {
            matches!(
                resolve(&tool, &[std::slice::from_ref(*b)], &store.connections),
                Err(ResolveError::CapabilityMismatch { .. }
                    | ResolveError::InputUnsupported { .. }
                    | ResolveError::InputRequired { .. })
            )
        })
    };
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
