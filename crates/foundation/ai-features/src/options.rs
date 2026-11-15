//! What a person can set up, and which tools each thing switches on.
//!
//! The launcher's Set up AI screen and an app's "no model yet" popup both start from this list, so
//! they agree. It is derived from the features, never written twice: a hosted service unlocks every
//! tool whose hosted routes name it, and a local model unlocks the tool whose recipe it is.

use crate::feature::Feature;
use craft_ai_core::PRESETS;
use serde::{Deserialize, Serialize};

/// How a thing is set up.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Route {
    /// Download model files for this feature and run them through ComfyUI on this computer.
    /// `recipe` is the workflow id of the model to set up, such as `sam3-segment`.
    Local { feature: String, recipe: String },
    /// Run a model in an app without ComfyUI. The engine has its own model files.
    Native {
        feature: String,
        provider: String,
        /// The model id the runtime's connection carries, so a saved connection for another model
        /// of the same runtime is not mistaken for this one.
        model: String,
    },
    /// Connect a hosted service. `preset` is a key of `craft_ai_core::PRESETS`.
    Service { preset: String },
}

/// One thing to set up.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupOption {
    /// Stable: `local:<feature>:<recipe>`, `native:<feature>:<provider>` or `service:<preset>`.
    pub id: String,
    pub title: String,
    /// One line about it. For a local model, only what its route says it cannot do (a built-in
    /// route that selects by clicking says so); otherwise empty, because the rest of the wording
    /// depends on the computer.
    pub note: String,
    pub route: Route,
    /// Ids of the features it switches on, in catalogue order.
    pub unlocks: Vec<String>,
    /// Apps can run it today. False for a model that can be downloaded but has no workflow to run it
    /// yet; the screen says so rather than promising a tool.
    pub usable: bool,
    /// How much a local model downloads, in bytes, so a popup can say so before it asks. `None`
    /// for a hosted service, which downloads nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

/// Every local model first, one per recipe ComfyUI runs in catalogue order, then every tool's built-in
/// runtimes (one per runtime, from its first usable recipe), then hosted services in the order the
/// features first name them.
pub fn setup_options(features: &[Feature]) -> Vec<SetupOption> {
    let mut locals = Vec::new();
    let mut natives = Vec::new();
    let mut services: Vec<SetupOption> = Vec::new();
    for feature in features {
        for recipe in feature.comfyui_recipes() {
            locals.push(SetupOption {
                id: format!("local:{}:{}", feature.id, recipe.workflow),
                title: recipe.label.clone(),
                note: String::new(),
                route: Route::Local {
                    feature: feature.id.clone(),
                    recipe: recipe.workflow.clone(),
                },
                unlocks: vec![feature.id.clone()],
                usable: recipe.usable,
                size_bytes: Some(recipe.total_bytes()),
            });
        }
        // Each built-in runtime is set up from its own recipe (`Feature::engine_recipe`), the one
        // planning and file setup use, so each runtime gets one option and following it cannot fail.
        for provider in feature.engines() {
            let Some(native) = feature
                .engine_recipe(provider)
                .and_then(|r| r.native.iter().find(|n| n.provider == provider))
                .filter(|n| !n.models.is_empty())
            else {
                continue;
            };
            natives.push(SetupOption {
                id: format!("native:{}:{}", feature.id, native.provider),
                title: native.label.clone(),
                note: native.note.clone().unwrap_or_default(),
                route: Route::Native {
                    feature: feature.id.clone(),
                    provider: native.provider.clone(),
                    model: native.model.clone(),
                },
                unlocks: vec![feature.id.clone()],
                usable: true,
                size_bytes: Some(native.total_bytes()),
            });
        }
        for option in &feature.cloud {
            let Some(preset) = PRESETS.iter().find(|p| p.key == option.preset) else {
                continue;
            };
            let id = format!("service:{}", preset.key);
            match services.iter_mut().find(|s| s.id == id) {
                Some(existing) => {
                    if !existing.unlocks.contains(&feature.id) {
                        existing.unlocks.push(feature.id.clone());
                    }
                }
                None => services.push(SetupOption {
                    id,
                    title: preset.name.to_owned(),
                    note: preset.note.to_owned(),
                    route: Route::Service {
                        preset: preset.key.to_owned(),
                    },
                    unlocks: vec![feature.id.clone()],
                    usable: true,
                    size_bytes: None,
                }),
            }
        }
    }
    locals.extend(natives);
    locals.extend(services);
    locals
}
