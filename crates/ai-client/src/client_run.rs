//! Planning and execution, with app scope and consent checked before side effects.
use std::sync::Arc;

use craft_ai_core::payload::Limits;
use craft_ai_core::{
    AiError, Connection, ConsentLedger, CostEstimate, Disclosure, ErrorKind, ModelId, Provider,
    Request, Role,
};
use craft_ai_native::secrets::secret_for;

use super::{AiClient, Job, NO_ADAPTER, Plan};
use crate::history;
use crate::route_support::{RouteQuery, RouteSupport};

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
        // The same for a pairing the adapter would refuse. The chosen model is left as it is.
        self.refuse_unsupported_route(resolved.connection, &resolved.model.id, &request)?;
        let mut disclosure = Disclosure::for_request(&self.app, resolved.connection, &request);
        crate::disclosed_size::measure_local_files(&mut disclosure, &request);
        let estimate = craft_ai_core::cost::estimate(
            resolved.model.pricing.as_ref(),
            &request,
            resolved.connection.billing,
        );
        Ok(Plan {
            connection: resolved.connection.clone(),
            model: resolved.model.id.clone(),
            request,
            disclosure,
            role_id: role.id.clone(),
            role_label: role.label.clone(),
            estimate,
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
                self.log_refused(plan, plan.estimate.clone(), &e);
                return Err(e);
            }
        };
        // The adapter's own figure wins where it gives one; otherwise the plan's, from pricing.
        let estimate = provider
            .estimate(&plan.model, &plan.request)
            .ok()
            .flatten()
            .or_else(|| plan.estimate.clone());
        // Only a rate limit is tried again: see `client_wait.rs` for why a dropped send is not.
        let submitted = self.submit_retry().run(
            super::wait::submit_worth_retrying,
            || provider.submit(&plan.model, &plan.request),
            std::thread::sleep,
        );
        match submitted {
            Ok(id) => {
                let recorder = self.history.as_ref().map(|file| {
                    Arc::new(history::begin(file, plan, &id, estimate, self.keep_prompts))
                });
                Ok(Job {
                    provider,
                    id,
                    recorder,
                    poll_retry: self.poll_retry(),
                })
            }
            Err(e) => {
                self.log_refused(plan, estimate, &e);
                Err(e)
            }
        }
    }

    /// Refuses a request the adapter has said it would refuse. The inputs, outputs, streaming and
    /// options are this request's, not the role's, because that is what would be sent.
    fn refuse_unsupported_route(
        &self,
        connection: &Connection,
        model: &ModelId,
        request: &Request,
    ) -> Result<(), AiError> {
        let inputs: Vec<_> = request.inputs.iter().map(|i| i.role.clone()).collect();
        let query = RouteQuery {
            connection,
            model,
            capability: &request.capability,
            inputs: &inputs,
            outputs: request.outputs,
            stream: request.stream,
            has_options: !request.params.is_empty(),
        };
        match self.factory.route_support(&query) {
            RouteSupport::Unsupported { reason } => Err(AiError::unsupported(reason)),
            RouteSupport::Supported | RouteSupport::Untested => Ok(()),
        }
    }

    /// Reads the key and builds the adapter.
    fn prepare(&self, plan: &Plan) -> Result<Arc<dyn Provider>, AiError> {
        // Without an adapter there is nothing to give the key to, so do not fetch it.
        if !self.factory.knows(&plan.connection.provider) {
            return Err(AiError::unsupported(NO_ADAPTER));
        }
        // A plan can be made by another client of this app whose build says otherwise, so this
        // build's own check runs again before the key is read.
        self.refuse_unsupported_route(&plan.connection, &plan.model, &plan.request)?;
        let secret = secret_for(&*self.secrets, &plan.connection)?;
        self.factory.build(&plan.connection, secret)
    }

    /// Records a provider-stage refusal only after app scope and consent have passed.
    fn log_refused(&self, plan: &Plan, estimate: Option<CostEstimate>, e: &AiError) {
        if let Some(file) = &self.history {
            history::refused(file, plan, estimate, self.keep_prompts, e);
        }
    }
}
