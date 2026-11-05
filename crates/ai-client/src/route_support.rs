//! Whether this build can actually carry out a request, as a fact separate from what is saved.
//!
//! A saved connection can declare that a model does a job, and a key can be saved beside it, while
//! the adapter refuses that model before sending anything. Reporting Ready on the saved facts alone
//! made a person pick a model, agree to send data, and only then hear it could not run.
//!
//! An adapter that knows its own limits says so here. This lives in the client, not on the core
//! `Provider` trait, so the shared contract does not widen, and asking never builds a provider,
//! reads a key, or contacts anything.
//!
//! Invariants: no answer means [`RouteSupport::Untested`], never supported. A check can only say
//! what it knows, so a custom adapter is not judged by a built-in's list. Nothing here chooses a
//! different model. A paid job is never used as a probe.

use std::sync::Arc;

use craft_ai_core::model::{InputRole, ModelInfo};
use craft_ai_core::{CapabilityId, Connection, ModelId};

/// What a person is told when an adapter refuses a pairing.
pub(crate) const ROUTE_UNSUPPORTED: &str =
    "This app can't use this model for this tool yet. Choose another in Set up AI.";

/// What this build knows about one request path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteSupport {
    /// The adapter has code for this model, job and set of inputs. It does not mean the account
    /// has access or that a real job has worked.
    Supported,
    /// The adapter would refuse this before sending anything. The reason is safe to show.
    Unsupported { reason: String },
    /// The adapter does not say. It may work; only a real job can show it.
    Untested,
}

/// The request path being asked about.
#[derive(Clone, Copy, Debug)]
pub struct RouteQuery<'a> {
    pub connection: &'a Connection,
    pub model: &'a ModelId,
    pub capability: &'a CapabilityId,
    pub inputs: &'a [InputRole],
}

/// An adapter's own answer. It must not read a key, build a provider, or contact anything.
pub type SupportFn = dyn Fn(&RouteQuery<'_>) -> RouteSupport + Send + Sync;

/// Saved and executable facts about the model a role is bound to, kept apart.
///
/// Whether the account may use the model is not held anywhere in this layer. Only a real job shows
/// it, and `last_success_ms` reports that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteFacts {
    /// The saved choice resolves against the saved connection, or why it does not.
    pub configured: Result<(), String>,
    /// Whether this build can carry it out. `Untested` while nothing is configured.
    pub executable: RouteSupport,
    /// When this connection and model last finished a real job here, in Unix milliseconds. `None`
    /// also covers a build with request history turned off.
    pub last_success_ms: Option<u64>,
}

fn unsupported() -> RouteSupport {
    RouteSupport::Unsupported {
        reason: ROUTE_UNSUPPORTED.to_owned(),
    }
}

/// The answer for an adapter that serves a fixed list of models and refuses anything else.
pub(crate) fn against_list(models: &[ModelInfo], query: &RouteQuery<'_>) -> RouteSupport {
    let Some(spec) = models
        .iter()
        .find(|m| &m.id == query.model)
        .and_then(|m| m.spec_for(query.capability))
    else {
        return unsupported();
    };
    let accepted = query.inputs.iter().all(|i| spec.accepts.contains_key(i));
    let complete = spec.requires.iter().all(|i| query.inputs.contains(i));
    if accepted && complete {
        RouteSupport::Supported
    } else {
        unsupported()
    }
}

/// Wraps a fixed list as a check. The list is read once, so asking every frame costs nothing.
pub(crate) fn list_check(models: Vec<ModelInfo>) -> Arc<SupportFn> {
    Arc::new(move |query| against_list(&models, query))
}

/// Vertex express serves one model, for one job, from a prompt alone.
pub(crate) fn vertex_check(query: &RouteQuery<'_>) -> RouteSupport {
    let only_a_prompt = query.inputs == [InputRole::Prompt];
    if query.model.as_str() == craft_ai_vertex::MODEL
        && query.capability.as_str() == "image.generate"
        && only_a_prompt
    {
        RouteSupport::Supported
    } else {
        unsupported()
    }
}
