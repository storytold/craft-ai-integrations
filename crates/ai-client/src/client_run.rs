//! Planning and execution, with app scope and consent checked before side effects.
use std::sync::Arc;

use craft_ai_core::payload::Limits;
use craft_ai_core::{AiError, ConsentLedger, Disclosure, ErrorKind, Provider, Request, Role};
use craft_ai_native::secrets::secret_for;

use super::{AiClient, Job, NO_ADAPTER, Plan};
use crate::history;

impl AiClient {
    /// Resolves the role and checks the request. Sends nothing and reads no key.
    pub fn plan(&self, role: &Role, request: Request) -> Result<Plan, AiError> {
        if request.capability != role.capability {
            return Err(AiError::invalid(format!(
                "this request is for {}, but \"{}\" needs {}",
                request.capability, role.label, role.capability
            )));
        }
        let store = self.shared.load()?;
        let resolved = store
            .resolve(&self.app, role)
            .map_err(|e| AiError::unsupported(e.to_string()))?;
        request.validate(resolved.spec, &Limits::default())?;
        // Refuse here, before anyone is asked to agree to send something this app cannot send.
        if !self.factory.knows(&resolved.connection.provider) {
            return Err(AiError::unsupported(NO_ADAPTER));
        }
        let disclosure = Disclosure::for_request(&self.app, resolved.connection, &request);
        Ok(Plan {
            connection: resolved.connection.clone(),
            model: resolved.model.id.clone(),
            request,
            disclosure,
            role_id: role.id.clone(),
            role_label: role.label.clone(),
        })
    }

    /// Starts the job. Refuses with [`ErrorKind::ConsentRequired`] if `ledger` does not cover the
    /// plan's disclosure. A plan for another app is an invalid request, refused before key access
    /// or provider and history side effects.
    pub fn start(&self, plan: &Plan, ledger: &ConsentLedger) -> Result<Job, AiError> {
        if plan.disclosure.app != self.app {
            return Err(AiError::invalid("this plan belongs to a different app"));
        }
        if plan.needs_consent(ledger) {
            return Err(AiError::new(
                ErrorKind::ConsentRequired,
                format!(
                    "sending to {} needs your agreement first",
                    plan.disclosure.destination
                ),
            ));
        }
        let provider = match self.prepare(plan) {
            Ok(provider) => provider,
            Err(e) => {
                self.log_refused(plan, None, &e);
                return Err(e);
            }
        };
        let estimate = provider.estimate(&plan.model, &plan.request).ok().flatten();
        match provider.submit(&plan.model, &plan.request) {
            Ok(id) => {
                let recorder = self.history.as_ref().map(|file| {
                    Arc::new(history::begin(file, plan, &id, estimate, self.keep_prompts))
                });
                Ok(Job {
                    provider,
                    id,
                    recorder,
                })
            }
            Err(e) => {
                self.log_refused(plan, estimate, &e);
                Err(e)
            }
        }
    }

    /// Reads the key and builds the adapter.
    fn prepare(&self, plan: &Plan) -> Result<Arc<dyn Provider>, AiError> {
        // Without an adapter there is nothing to give the key to, so do not fetch it.
        if !self.factory.knows(&plan.connection.provider) {
            return Err(AiError::unsupported(NO_ADAPTER));
        }
        let secret = secret_for(&*self.secrets, &plan.connection)?;
        self.factory.build(&plan.connection, secret)
    }

    /// Records a provider-stage refusal only after app scope and consent have passed.
    fn log_refused(&self, plan: &Plan, estimate: Option<craft_ai_core::CostEstimate>, e: &AiError) {
        if let Some(file) = &self.history {
            history::refused(file, plan, estimate, self.keep_prompts, e);
        }
    }
}
