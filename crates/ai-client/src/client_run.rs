//! Planning and execution, with app scope and consent checked before side effects.
use std::sync::Arc;

use craft_ai_core::payload::Limits;
use craft_ai_core::{
    AiError, Connection, ConsentLedger, CostEstimate, Disclosure, ErrorKind, Locality, ModelId,
    Provider, Request, Role,
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
        // A workflow on ComfyUI on this computer can still pass the picture and prompt to an
        // add-on step that sends them elsewhere, so such steps are named and the person asked.
        let steps = self.unknown_steps(resolved.connection, &resolved.model.id);
        if !steps.is_empty() {
            disclosure.locality = Locality::Remote;
            disclosure.unknown_steps = steps;
        }
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
    ///
    /// Blocks the calling thread while it pauses between sends the service refused with a rate
    /// limit (up to the send rule's budget, 30 seconds by default; see
    /// [`with_retries`](Self::with_retries)). Call it from a worker, or turn the send retry off
    /// with [`Backoff::NEVER`](craft_ai_core::Backoff::NEVER) before calling it from the
    /// interface thread.
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

    /// Checks the executable route and workflow snapshot before reading the key, then builds
    /// from that checked snapshot or the app's registered adapter.
    fn prepare(&self, plan: &Plan) -> Result<Arc<dyn Provider>, AiError> {
        // Without an adapter there is nothing to give the key to, so do not fetch it.
        if !self.factory.knows(&plan.connection.provider) {
            return Err(AiError::unsupported(NO_ADAPTER));
        }
        // A plan can be made by another client of this app whose build says otherwise, so this
        // build's own check runs again before the key is read.
        self.refuse_unsupported_route(&plan.connection, &plan.model, &plan.request)?;
        // Freeze the built-in workflow before the key is read: another import during that
        // lookup must not replace the graph whose steps were checked for this job.
        let snapshot = if is_comfyui(&plan.connection) {
            self.factory
                .prepare_comfyui_agreed(&plan.model, &plan.disclosure.unknown_steps)
                .transpose()?
        } else {
            None
        };
        // App-owned adapters keep their original build path and the step comparison used by
        // plans. Their own builder is never replaced by the built-in snapshot provider.
        if snapshot.is_none()
            && self.unknown_steps(&plan.connection, &plan.model) != plan.disclosure.unknown_steps
        {
            return Err(AiError::invalid(crate::factory::WORKFLOW_CHANGED));
        }
        let secret = secret_for(&*self.secrets, &plan.connection)?;
        match snapshot {
            Some(snapshot) => snapshot.build(&self.factory, &plan.connection),
            None => self.factory.build(&plan.connection, secret),
        }
    }

    /// The steps of a ComfyUI workflow that are not known to keep data on this computer; empty
    /// for every other kind of connection, and when the workflow cannot be read.
    fn unknown_steps(&self, connection: &Connection, model: &ModelId) -> Vec<String> {
        // Match the factory's case-insensitive name so a mixed-case connection cannot skip
        // the agreement required for the same workflow.
        if !is_comfyui(connection) {
            return Vec::new();
        }
        self.factory
            .comfyui_unknown_steps(model)
            .unwrap_or_default()
    }

    /// Records a provider-stage refusal only after app scope and consent have passed.
    fn log_refused(&self, plan: &Plan, estimate: Option<CostEstimate>, e: &AiError) {
        if let Some(file) = &self.history {
            history::refused(file, plan, estimate, self.keep_prompts, e);
        }
    }
}

/// Whether `connection` uses the ComfyUI adapter. The factory matches names without regard to
/// case, so this does too: a mixed-case name must not skip the workflow-step agreement.
fn is_comfyui(connection: &Connection) -> bool {
    connection.provider.eq_ignore_ascii_case("comfyui")
}
