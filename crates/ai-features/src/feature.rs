//! The AI tools a person can switch on, and where they would appear.

use crate::recipe::{CloudOption, LocalRecipe};
use craft_ai_core::{CapabilityId, InputRole, Role, RoleId};
use serde::{Deserialize, Serialize};

/// Where a tool stands in one app. Nothing is `Shipped` until an app really calls the shared layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppStatus {
    /// Suggested from the app's description. The app's owner has not confirmed it.
    Proposed,
    /// The owner has agreed to it.
    Planned,
    /// The app calls the shared layer for it today.
    Shipped,
}

/// One place a feature shows up.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppUse {
    pub app: String,
    pub tool: String,
    pub status: AppStatus,
}

/// A tool the user can enable once and use across the suite.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feature {
    pub id: String,
    pub title: String,
    pub summary: String,
    /// What a provider must advertise to run it.
    pub capability: CapabilityId,
    /// What the tool sends, so a model that cannot take these is not offered for it.
    #[serde(default)]
    pub inputs: Vec<InputRole>,
    pub uses: Vec<AppUse>,
    /// Run it on this computer, when recipes have been worked out and checked. A tool may offer
    /// several models; the first is the one a plain `plan` call refers to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locals: Vec<LocalRecipe>,
    /// Hosted routes that have a working adapter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cloud: Vec<CloudOption>,
}

impl Feature {
    /// The role the launcher binds for this tool. Apps declare a role with the same id to inherit
    /// the launcher's choice; an app's own binding still wins.
    pub fn role(&self) -> Option<Role> {
        Some(Role {
            id: RoleId::new(&self.id).ok()?,
            label: self.title.clone(),
            capability: self.capability.clone(),
            inputs: self.inputs.clone(),
        })
    }

    /// The first recipe apps can run. A plain built-in Install offer and engine advice's hardware
    /// limits use it.
    pub fn first_usable_recipe(&self) -> Option<&LocalRecipe> {
        self.locals.iter().find(|r| r.usable)
    }

    /// The recipes ComfyUI runs, in catalogue order. A recipe that is built in only
    /// ([`LocalRecipe::comfyui`] false) is never one of them.
    pub fn comfyui_recipes(&self) -> impl Iterator<Item = &LocalRecipe> {
        self.locals.iter().filter(|r| r.comfyui)
    }

    /// The recipe the built-in engine `provider` acts on: the first one apps can run that lists
    /// that engine. Planning, setup options, the Install offer for that engine, file setup and
    /// registration all use it, so an option the picker shows is never one that setup then
    /// refuses. Each engine follows its own recipe, so a first recipe without Candle does not hide
    /// a later one with it.
    pub fn engine_recipe(&self, provider: &str) -> Option<&LocalRecipe> {
        self.locals
            .iter()
            .find(|r| r.usable && r.native.iter().any(|n| n.provider == provider))
    }

    /// Every built-in engine some usable recipe lists, once each, in catalogue order.
    pub fn engines(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for route in self
            .locals
            .iter()
            .filter(|r| r.usable)
            .flat_map(|r| &r.native)
        {
            if !out.contains(&route.provider.as_str()) {
                out.push(&route.provider);
            }
        }
        out
    }

    /// Apps where the tool exists today.
    pub fn shipped_in(&self) -> impl Iterator<Item = &AppUse> {
        self.uses.iter().filter(|u| u.status == AppStatus::Shipped)
    }
}
