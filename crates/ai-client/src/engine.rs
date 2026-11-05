//! Suggests which local engine suits this computer for a tool. Offers only; nothing is bound, so
//! a person's own pick (`ToolDesk::choose`, `AiClient::bind`) is never touched.

use craft_ai_core::{AiError, Role};
use craft_ai_features::{EngineAdvice, EngineOffer, Feature, SetupOption, SystemProfile, advise};

use crate::client::AiClient;
use crate::picker::{GroupState, PickerView};

impl AiClient {
    /// The local engines `feature` can run on, best first, judged for `profile` (see
    /// `craft_ai_comfy_env::system::detect`). An engine this app has no adapter for is listed but
    /// never suggested. Every built-in engine some usable recipe lists is offered (each runs its own
    /// recipe, [`Feature::engine_recipe`]), then ComfyUI when a usable recipe runs on it; the
    /// memory limit is the first usable
    /// recipe's, the one [`ToolDesk::enable_local`](crate::ToolDesk::enable_local) registers. A tool
    /// with no usable recipe has no advice.
    pub fn engine_advice(&self, feature: &Feature, profile: &SystemProfile) -> Vec<EngineAdvice> {
        let Some(recipe) = feature.first_usable_recipe() else {
            return Vec::new();
        };
        let mut providers: Vec<&str> = feature.engines();
        if feature.comfyui_recipes().any(|r| r.usable) {
            providers.push("comfyui");
        }
        let offers: Vec<EngineOffer> = providers
            .into_iter()
            .map(|p| EngineOffer {
                provider: p.to_owned(),
                adapter_present: self.has_adapter(p),
                accelerators: self.declared_accelerators(p),
                fast_on_processor: feature
                    .engine_recipe(p)
                    .and_then(|r| r.native.iter().find(|n| n.provider == p))
                    .is_some_and(|n| n.fast_on_processor),
            })
            .collect();
        advise(profile, &offers, recipe.min_vram_mb)
    }
}

impl AiClient {
    /// [`picker`](Self::picker) with the provider that `advice` recommends marked `suggested`. Only
    /// a provider that is ready can be marked, the person's own choice is left selected, and
    /// nothing is written.
    pub fn picker_with_advice(
        &self,
        role: &Role,
        options: &[SetupOption],
        advice: &[EngineAdvice],
    ) -> Result<PickerView, AiError> {
        let mut view = self.picker(role, options)?;
        let Some(recommended) = advice.iter().find(|a| a.recommended) else {
            return Ok(view);
        };
        let store = self.shared.load()?;
        for group in &mut view.groups {
            group.suggested = group.state == GroupState::Ready
                && group.models.iter().any(|m| {
                    store
                        .connections
                        .iter()
                        .any(|c| c.id == m.connection && c.provider == recommended.provider)
                });
        }
        Ok(view)
    }
}
