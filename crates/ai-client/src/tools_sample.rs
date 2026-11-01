//! Sample planning and approval, bound to the disclosure the person reviewed.
use craft_ai_core::{AiError, ConsentLedger, Disclosure, Request, Role};
use craft_ai_features::{Feature, sample_request_for};
use craft_ai_native::ConsentFile;

use super::{TestStart, ToolDesk};

impl ToolDesk {
    /// Plans a sample without approval when `reviewed` is `None`. Show the returned
    /// [`TestStart::NeedsConsent`] disclosure, retain it, and pass `Some(&disclosure)` only
    /// after the person agrees. If the current disclosure differs, asks again without saving
    /// consent or building a provider. An existing agreement can still cover the new plan.
    pub fn start_test(
        &self,
        role: &Role,
        request: Request,
        reviewed: Option<&Disclosure>,
    ) -> Result<TestStart, AiError> {
        let plan = self.client.plan(role, request)?;
        let mut ledger = self
            .consent
            .as_ref()
            .map_or_else(ConsentLedger::default, ConsentFile::load_or_empty);
        if plan.needs_consent(&ledger) {
            if reviewed != Some(plan.disclosure()) {
                return Ok(TestStart::NeedsConsent {
                    disclosure: plan.disclosure().clone(),
                    connection_label: plan.connection_label().to_owned(),
                    billing: plan.connection.billing,
                });
            }
            ledger.grant(plan.disclosure());
            if let Some(file) = &self.consent {
                file.save(&ledger)?;
            }
        }
        Ok(TestStart::Started(self.client.start(&plan, &ledger)?))
    }

    /// Starts the sample matching the current route, with a click for built-in selection models
    /// and a description otherwise. `reviewed` follows [`start_test`](Self::start_test): retain
    /// the disclosure returned by the initial `None` call and pass it back after agreement.
    /// A changed route or payload must have the same disclosure or ask again.
    pub fn start_sample(
        &self,
        feature: &Feature,
        reviewed: Option<&Disclosure>,
    ) -> Result<TestStart, AiError> {
        let role = feature
            .role()
            .ok_or_else(|| AiError::invalid("the tool id is not valid"))?;
        // A custom connection may reuse the engine name or replace its model. Only the exact
        // built-in recipe gets the click sample that a built-in selection model reads.
        let built_in = match self.client.local_route_for(&role) {
            Ok(Some(route)) => feature
                .engine_recipe(&route.provider)
                .and_then(|recipe| {
                    recipe
                        .native
                        .iter()
                        .find(|native| native.provider == route.provider)
                })
                .is_some_and(|native| {
                    route.connection.as_str()
                        == crate::local::native_connection_id(&feature.id, &route.provider)
                        && route.model.as_str() == native.model
                }),
            _ => false,
        };
        let request = sample_request_for(feature, built_in)
            .ok_or_else(|| AiError::unsupported("there is no test for this tool yet"))?;
        self.start_test(&role, request, reviewed)
    }
}
