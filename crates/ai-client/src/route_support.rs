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

use std::collections::BTreeMap;
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
    /// How many results the request asks for. Status and the picker ask about one.
    pub outputs: u32,
    /// Whether the request asks for incremental results. Status and the picker ask about none.
    pub stream: bool,
    /// Whether the request carries options (`params`). Status and the picker ask about none.
    pub has_options: bool,
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

/// The refusal with the reason every built-in list check gives.
fn unsupported() -> RouteSupport {
    RouteSupport::Unsupported {
        reason: ROUTE_UNSUPPORTED.to_owned(),
    }
}

/// The answer for an adapter that serves a fixed list of models and refuses anything else. Each
/// input role is counted against the listed model's limit, and outputs and streaming are checked
/// against it too, because `submit` validates the request against the list, not against what the
/// saved connection declares. Options are refused: none of these adapters maps them yet.
pub(crate) fn against_list(models: &[ModelInfo], query: &RouteQuery<'_>) -> RouteSupport {
    let Some(spec) = models
        .iter()
        .find(|m| &m.id == query.model)
        .and_then(|m| m.spec_for(query.capability))
    else {
        return unsupported();
    };
    let mut counts: BTreeMap<&InputRole, u32> = BTreeMap::new();
    for role in query.inputs {
        *counts.entry(role).or_insert(0) += 1;
    }
    let accepted = counts
        .iter()
        .all(|(role, n)| spec.accepts.get(*role).is_some_and(|max| n <= max));
    let complete = spec.requires.iter().all(|i| query.inputs.contains(i));
    let outputs = (1..=spec.max_outputs).contains(&query.outputs);
    let stream = !query.stream || spec.features.streaming;
    if accepted && complete && outputs && stream && !query.has_options {
        RouteSupport::Supported
    } else {
        unsupported()
    }
}

/// An adapter's rule for which saved addresses its constructor accepts.
pub(crate) type AddressRule = fn(&str) -> bool;

/// What a person is told when a built-in adapter would refuse the saved address.
pub(crate) const ADDRESS_UNSUPPORTED: &str =
    "This app can't use this connection's address. Check it in Set up AI.";

/// Puts an adapter's address rule in front of `check`, so a connection whose saved address the
/// adapter refuses when it is built is reported before anyone agrees to send. No address is refused
/// too, because the built-in adapters need one.
pub(crate) fn at_address(allowed: AddressRule, check: Arc<SupportFn>) -> Arc<SupportFn> {
    Arc::new(move |query| {
        if query.connection.endpoint.as_deref().is_some_and(allowed) {
            check(query)
        } else {
            RouteSupport::Unsupported {
                reason: ADDRESS_UNSUPPORTED.to_owned(),
            }
        }
    })
}

/// Wraps a fixed list as a check. The list is read once, so asking every frame costs nothing.
pub(crate) fn list_check(models: Vec<ModelInfo>) -> Arc<SupportFn> {
    Arc::new(move |query| against_list(&models, query))
}

/// Vertex express serves one model, for one job, from a prompt alone, with one output, no
/// streaming and no options.
pub(crate) fn vertex_check(query: &RouteQuery<'_>) -> RouteSupport {
    let only_a_prompt = query.inputs == [InputRole::Prompt]
        && query.outputs == 1
        && !query.stream
        && !query.has_options;
    if query.model.as_str() == craft_ai_vertex::MODEL
        && query.capability.as_str() == "image.generate"
        && only_a_prompt
    {
        RouteSupport::Supported
    } else {
        unsupported()
    }
}

/// Gemini fills only: one source, one mask, at most one prompt, any number of reference pictures
/// (the service says how many its model takes), one output, no streaming. Any well-formed Gemini image model is taken, because
/// `submit` takes one (see `GeminiProvider::model_allowed`), so a list check would refuse a newer
/// model that works.
/// Options are allowed: `image_size` is the one Gemini reads, and `submit` checks its value.
pub(crate) fn gemini_check(query: &RouteQuery<'_>) -> RouteSupport {
    let count = |role: InputRole| query.inputs.iter().filter(|r| **r == role).count();
    let only_fill_inputs = query.inputs.iter().all(|r| {
        matches!(
            r,
            InputRole::Source | InputRole::Mask | InputRole::Prompt | InputRole::Reference
        )
    });
    let fill = only_fill_inputs
        && count(InputRole::Source) == 1
        && count(InputRole::Mask) == 1
        && count(InputRole::Prompt) <= 1
        && query.outputs == 1
        && !query.stream;
    if fill
        && query.capability.as_str() == "image.inpaint"
        && craft_ai_gemini::GeminiProvider::model_allowed(query.model.as_str())
    {
        RouteSupport::Supported
    } else {
        unsupported()
    }
}
