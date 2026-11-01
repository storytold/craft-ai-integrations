//! A tool's local model becomes something an app can read.
//!
//! The catalogue says a tool has a local recipe (SAM 3 for Select subject), and the launcher shows
//! it Ready once the files and node pack are in place. Until now nothing in the shared store
//! represented that, so no tool could be bound to it and no app could read a choice. This writes
//! the missing piece: a local connection that offers the recipe's model for the tool, and binds
//! the tool to it unless a person has already chosen something else.

use std::path::{Path, PathBuf};

use craft_ai_core::endpoint::Endpoint;
use craft_ai_core::role::ResolveError;
use craft_ai_core::{
    AiError, Auth, Billing, Binding, CapabilitySpec, Connection, ConnectionId, Locality, ModelId,
    ModelInfo, Role,
};
use craft_ai_features::Feature;

use crate::client::AiClient;
use crate::folders::CheckedNative;
use crate::tools::ToolDesk;

/// The exact local connection and model chosen for a tool, without any key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalRoute {
    pub connection: ConnectionId,
    pub model: ModelId,
    pub provider: String,
    pub endpoint: Option<String>,
    /// The weights file of a model an app runs itself, as recorded by whoever registered it.
    pub file: Option<String>,
}

/// What [`ToolDesk::adopt`] did with files that passed their checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adopted {
    pub connection: ConnectionId,
    pub model: ModelId,
    /// The tool is now bound to it. False when a choice was already made, which is left alone.
    pub bound: bool,
    /// The choice that stayed in place, when `bound` is false. Show it and ask before switching:
    /// [`ToolDesk::choose`] with `connection` and `model` makes this one the choice.
    pub kept: Option<(ConnectionId, ModelId)>,
}

/// What [`ToolDesk::enable_local`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalEnabled {
    pub connection: ConnectionId,
    pub model: ModelId,
    /// The tool was bound to it now. False when a choice was already made, which is left alone.
    pub bound: bool,
}

/// The id of the connection [`ToolDesk::enable_native`] saves for the built-in runtime `provider`
/// of `feature`, so the picker can tell which model a saved connection stands for.
pub(crate) fn native_connection_id(feature: &str, provider: &str) -> String {
    format!("local-{feature}-{provider}")
}

/// Whether `connection` is one a local recipe of `feature` registered through
/// [`ToolDesk::enable_local`] or [`ToolDesk::enable_local_recipe`]: the ComfyUI connection
/// `local-<feature>` or one named `local-<feature>-recipe-<workflow>`. Its model id is the
/// recipe's workflow, so the picker can tell which recipe it stands for.
pub(crate) fn is_recipe_connection(connection: &Connection, feature: &str) -> bool {
    let id = connection.id.as_str();
    connection.provider == "comfyui"
        && (id == format!("local-{feature}") || id.starts_with(&format!("local-{feature}-recipe-")))
}

impl ToolDesk {
    /// Registers the first local model of `feature` that ComfyUI can run, served by the ComfyUI
    /// at `endpoint` (a loopback address), and binds the tool to it if nothing is chosen yet.
    pub fn enable_local(&self, feature: &Feature, endpoint: &str) -> Result<LocalEnabled, AiError> {
        let recipe = feature
            .comfyui_recipes()
            .find(|r| r.usable)
            .ok_or_else(|| {
                AiError::unsupported(format!(
                    "{} has no local model that can run yet",
                    feature.title
                ))
            })?;
        self.enable_recipe(feature, recipe, endpoint)
    }

    /// Registers one named local model of `feature` (`workflow` is its recipe's workflow id, such
    /// as `sam3-segment`), served by the ComfyUI at `endpoint`, and binds the tool to it if
    /// nothing is chosen yet. Refused, with nothing written, when the feature has no recipe of
    /// that name that can run.
    pub fn enable_local_recipe(
        &self,
        feature: &Feature,
        workflow: &str,
        endpoint: &str,
    ) -> Result<LocalEnabled, AiError> {
        let recipe = feature
            .comfyui_recipes()
            .find(|r| r.workflow == workflow && r.usable)
            .ok_or_else(|| {
                AiError::unsupported(format!(
                    "{} has no local model called {workflow} that can run yet",
                    feature.title
                ))
            })?;
        self.enable_recipe(feature, recipe, endpoint)
    }

    /// One recipe becomes one connection offering its model. The plain `local-<feature>` id goes
    /// to the recipe that already holds it in the store, or, when no connection has it yet, to the
    /// first recipe. So a choice saved by an earlier build, when another recipe came first (SAM 3
    /// before SAM 2.1), still points at its model, and that recipe keeps updating the same
    /// connection. Every other recipe gets `local-<feature>-recipe-<workflow>`: built-in runtimes
    /// already use `local-<feature>-<provider>`, and a recipe named like one (`candle`) must never
    /// replace it.
    fn enable_recipe(
        &self,
        feature: &Feature,
        recipe: &craft_ai_features::LocalRecipe,
        endpoint: &str,
    ) -> Result<LocalEnabled, AiError> {
        let role = feature
            .role()
            .ok_or_else(|| AiError::invalid("the tool id is not valid"))?;
        let address = local_address(endpoint)?;
        let plain = format!("local-{}", feature.id);
        let store = self.shared.load()?;
        let primary = match store.connections.iter().find(|c| c.id.as_str() == plain) {
            Some(held) => {
                held.provider == "comfyui"
                    && held.models.iter().any(|m| m.id.as_str() == recipe.workflow)
            }
            None => feature
                .comfyui_recipes()
                .next()
                .is_some_and(|first| first.workflow == recipe.workflow),
        };
        let id = if primary {
            ConnectionId::new(plain)
        } else {
            ConnectionId::new(format!("local-{}-recipe-{}", feature.id, recipe.workflow))
        }
        .map_err(|e| AiError::invalid(e.to_string()))?;
        let model_id =
            ModelId::new(&recipe.workflow).map_err(|e| AiError::invalid(e.to_string()))?;
        let model = tool_model(feature, model_id.clone(), &recipe.label);
        let label = format!("{} on this computer", recipe.label);
        let connection = local_connection(id, label, "comfyui", Some(address), model);
        self.register(connection, role.id, model_id)
    }

    /// Saves `connection` (replacing one with the same id and the same adapter) and binds `role`
    /// to it unless a choice is already made. A choice already made, even one that no longer
    /// works, is never replaced. A connection of another adapter under the same id is refused and
    /// left untouched, because its binding would then point at a different route.
    fn register(
        &self,
        connection: Connection,
        role: craft_ai_core::RoleId,
        model: ModelId,
    ) -> Result<LocalEnabled, AiError> {
        let id = connection.id.clone();
        let binding = Binding {
            role,
            connection: id.clone(),
            model: model.clone(),
        };
        let mut bound = false;
        let clash = self.shared.update(|store| {
            match store.connections.iter_mut().find(|c| c.id == id) {
                // The same id with another adapter is another route that happens to share the
                // name (a custom runtime called `recipe-x` against a recipe called `x`).
                // Replacing it would swap the route under a binding that still names its model.
                Some(existing) if existing.provider != connection.provider => {
                    return Some(existing.provider.clone());
                }
                Some(existing) => *existing = connection,
                None => store.connections.push(connection),
            }
            if !store.defaults.iter().any(|b| b.role == binding.role) {
                store.defaults.push(binding);
                bound = true;
            }
            None
        })?;
        if let Some(provider) = clash {
            return Err(AiError::invalid(format!(
                "the connection {id} already belongs to a {provider} route and was left as it is"
            )));
        }
        Ok(LocalEnabled {
            connection: id,
            model,
            bound,
        })
    }
}

impl ToolDesk {
    /// Makes files that passed every check a model the tool can use. Only a [`CheckedNative`]
    /// is accepted, so a path nobody hashed cannot be registered through the setup flow. The tool
    /// is bound only if no choice is made yet; otherwise the earlier choice is returned in
    /// [`Adopted::kept`] and nothing is switched without the person's yes.
    pub fn adopt(&self, checked: &CheckedNative) -> Result<Adopted, AiError> {
        let enabled =
            self.enable_native_with(checked.feature(), checked.provider(), checked.weights())?;
        let kept = if enabled.bound {
            None
        } else {
            let role = checked
                .feature()
                .role()
                .ok_or_else(|| AiError::invalid("the tool id is not valid"))?;
            self.view(&role)?.bound
        };
        Ok(Adopted {
            connection: enabled.connection,
            model: enabled.model,
            bound: enabled.bound,
            kept,
        })
    }

    /// Registers the first built-in runtime ([`NativeRoute`](craft_ai_features::NativeRoute)) of
    /// `feature`'s first recipe that can run, reading the weights at `weights`, and binds the tool
    /// to it if nothing is chosen yet.
    pub fn enable_native(
        &self,
        feature: &Feature,
        weights: &Path,
    ) -> Result<LocalEnabled, AiError> {
        let first = feature
            .locals
            .iter()
            .find(|r| r.usable)
            .and_then(|r| r.native.first())
            .map(|n| n.provider.clone())
            .ok_or_else(|| {
                AiError::unsupported(format!("{} has no built-in runtime", feature.title))
            })?;
        self.enable_native_with(feature, &first, weights)
    }

    /// Registers the built-in runtime named `provider` (`rten`, `candle`, `burn`) from that
    /// engine's recipe ([`Feature::engine_recipe`]), the same one setup and file checks use. Only
    /// a runtime some usable recipe lists is accepted, matched exactly. Every runtime reads the same
    /// weights file and gets its own connection; the tool is bound only if nothing is chosen yet.
    pub fn enable_native_with(
        &self,
        feature: &Feature,
        provider: &str,
        weights: &Path,
    ) -> Result<LocalEnabled, AiError> {
        let native = feature
            .engine_recipe(provider)
            .and_then(|r| r.native.iter().find(|n| n.provider == provider))
            .ok_or_else(|| {
                AiError::unsupported(format!(
                    "{} has no built-in {provider} runtime",
                    feature.title
                ))
            })?;
        let role = feature
            .role()
            .ok_or_else(|| AiError::invalid("the tool id is not valid"))?;
        let file = weights_file(weights)?;
        let id = ConnectionId::new(native_connection_id(&feature.id, &native.provider))
            .map_err(|e| AiError::invalid(e.to_string()))?;
        let model_id = ModelId::new(&native.model).map_err(|e| AiError::invalid(e.to_string()))?;
        let mut model = tool_model(feature, model_id.clone(), &native.label);
        model.local_file = Some(file);
        let connection = local_connection(id, native.label.clone(), &native.provider, None, model);
        self.register(connection, role.id, model_id)
    }
}

impl AiClient {
    /// The weights file for a tool whose chosen model an app runs itself with `provider`.
    ///
    /// `None` only when nothing is bound. A choice run by another adapter, one with no file
    /// recorded, or a file that is gone is an error, never a cue to look elsewhere.
    pub fn local_file_for(&self, role: &Role, provider: &str) -> Result<Option<PathBuf>, AiError> {
        let Some(route) = self.local_route_for(role)? else {
            return Ok(None);
        };
        if route.provider != provider {
            return Err(AiError::unsupported(
                "The model you chose can't be run by this app yet. Choose another in Set up AI.",
            ));
        }
        let file = route.file.ok_or_else(|| {
            AiError::invalid("The model you chose isn't fully set up. Open Set up AI to fix it.")
        })?;
        let path = PathBuf::from(file);
        if !path.is_file() {
            return Err(AiError::unsupported(
                "The file for the model you chose is no longer there. Open Set up AI to fix it.",
            ));
        }
        Ok(Some(path))
    }

    /// The selected local inference route, with the exact adapter name and model. An unbound
    /// tool returns `None`; a broken or remote choice is an error, never a cue to fall back.
    pub fn local_route_for(&self, role: &Role) -> Result<Option<LocalRoute>, AiError> {
        let store = self.shared.load()?;
        let resolved = match store.resolve(self.app(), role) {
            Ok(route) => route,
            Err(ResolveError::Unbound(_)) => return Ok(None),
            Err(error) => return Err(AiError::unsupported(error.to_string())),
        };
        if resolved.connection.locality != Locality::Local {
            return Err(AiError::unsupported(
                "The model you chose runs online, not on this computer.",
            ));
        }
        Ok(Some(LocalRoute {
            connection: resolved.connection.id.clone(),
            model: resolved.model.id.clone(),
            provider: resolved.connection.provider.clone(),
            endpoint: resolved.connection.endpoint.clone(),
            file: resolved.model.local_file.clone(),
        }))
    }

    /// Address for a local ComfyUI session (PhotoCraft's hover path). Only `None` when nothing
    /// is bound. A chosen native, remote or broken route is an error, never a signal to run the
    /// old hard-coded ComfyUI address. The exact model is in [`local_route_for`](Self::local_route_for).
    pub fn endpoint_for(&self, role: &Role) -> Result<Option<String>, AiError> {
        let Some(route) = self.local_route_for(role)? else {
            return Ok(None);
        };
        if route.provider != "comfyui" {
            return Err(AiError::unsupported(
                "The model you chose can't be run by this app yet. Choose another in Set up AI.",
            ));
        }
        route.endpoint.map(Some).ok_or_else(|| {
            AiError::invalid("The model you chose isn't fully set up. Open Set up AI to fix it.")
        })
    }
}

/// The model as the tool sees it: what it takes is what the tool sends, as the catalogue declares it.
fn tool_model(feature: &Feature, id: ModelId, label: &str) -> ModelInfo {
    let mut spec = CapabilitySpec::new(feature.capability.clone());
    for input in &feature.inputs {
        spec = spec.accepting(input.clone(), 1);
    }
    ModelInfo::new(id, label).with(spec)
}

/// A connection to a model on this computer: free, and no key.
fn local_connection(
    id: ConnectionId,
    label: String,
    provider: &str,
    endpoint: Option<String>,
    model: ModelInfo,
) -> Connection {
    Connection {
        id,
        label,
        provider: provider.to_owned(),
        endpoint,
        locality: Locality::Local,
        auth: Auth::None,
        billing: Billing::Free,
        models: vec![model],
        enabled: true,
        key_header: None,
    }
}

/// An absolute path to an existing `.safetensors`, `.onnx` or `.bpk` file, as text. The last is the
/// saved copy the Burn engine reads, made once on this computer from the published `.onnx` files.
fn weights_file(path: &Path) -> Result<String, AiError> {
    let text = path
        .to_str()
        .filter(|t| !t.contains('\0'))
        .ok_or_else(|| AiError::invalid("the weights path is not valid text"))?;
    let known = path.extension().is_some_and(|e| {
        ["safetensors", "onnx", "bpk"]
            .iter()
            .any(|known| e.eq_ignore_ascii_case(known))
    });
    if !path.is_absolute() || !known || !path.is_file() {
        return Err(AiError::invalid(
            "the weights must be an existing .safetensors, .onnx or .bpk file, given by its full path",
        ));
    }
    Ok(text.to_owned())
}

/// A loopback address with nothing after the port, or an error. A local model is only ever
/// reached on this computer.
fn local_address(input: &str) -> Result<String, AiError> {
    let trimmed = input.trim_end_matches('/');
    let endpoint = Endpoint::parse(trimmed)
        .map_err(|_| AiError::invalid("the local address is not a valid address"))?;
    if !endpoint.is_loopback() || trimmed.contains(['?', '#']) {
        return Err(AiError::invalid(
            "a local model must be reached on this computer, without a query or fragment",
        ));
    }
    Ok(trimmed.to_owned())
}
