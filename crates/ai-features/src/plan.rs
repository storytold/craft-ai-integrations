//! Compares a recipe with what this computer already has and says what is left to do.
//!
//! Pure: the caller builds an [`Inventory`] (the native side scans the disk) and gets a [`Plan`].
//! A file counts as present only when folder, name and exact size all match, so a half-finished
//! or different file with the right name is treated as missing.

use crate::feature::Feature;
use crate::recipe::{Licence, LocalRecipe, ModelFile, NodePack};
use serde::{Deserialize, Serialize};

/// A model file found on disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledFile {
    pub folder: String,
    pub filename: String,
    pub size_bytes: u64,
}

/// What this computer has. Every field is what was found, never what was assumed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    /// A ComfyUI installation was found.
    pub comfyui_found: bool,
    pub files: Vec<InstalledFile>,
    /// Folder names under `custom_nodes`, lower case.
    pub node_folders: Vec<String>,
    pub vram_mb: Option<u32>,
    pub free_disk_bytes: Option<u64>,
}

impl Inventory {
    /// The installed copy of a wanted file: same folder, same size, and a path that is the wanted
    /// name either directly or inside a sub-folder (a user may keep it in `FLUX/` and so on).
    pub fn find(&self, wanted: &ModelFile) -> Option<&InstalledFile> {
        let leaf = wanted.filename.to_ascii_lowercase();
        self.files.iter().find(|f| {
            let path = f.filename.to_ascii_lowercase();
            f.folder.eq_ignore_ascii_case(&wanted.folder)
                && f.size_bytes == wanted.size_bytes
                && (path == leaf || path.ends_with(&format!("/{leaf}")))
        })
    }

    fn has_pack(&self, pack: &NodePack) -> bool {
        pack.folder_names
            .iter()
            .any(|n| self.node_folders.iter().any(|f| f.eq_ignore_ascii_case(n)))
    }
}

/// Why a feature cannot be set up here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Blocker {
    NotEnoughVram { needed_mb: u32, have_mb: u32 },
    NotEnoughDisk { needed_bytes: u64, free_bytes: u64 },
}

/// Where a feature stands on this computer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    /// Nobody has worked out how to run it locally yet.
    NoLocalRoute,
    /// ComfyUI was not found.
    NoRuntime,
    /// Everything it needs is here.
    Ready,
    /// Something has to be installed first.
    NeedsInstall,
    /// It cannot run here as things stand.
    Blocked,
}

/// The result of comparing one feature with one inventory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub feature: String,
    /// The recipe this plan is about: its workflow id, such as `sam3-segment`. Empty for a
    /// feature with no local route.
    #[serde(default)]
    pub recipe: String,
    pub state: State,
    pub missing_files: Vec<ModelFile>,
    pub missing_packs: Vec<NodePack>,
    /// What would be downloaded.
    pub download_bytes: u64,
    /// Licences the user has to accept before the downloads start, once each.
    pub licences: Vec<Licence>,
    pub blockers: Vec<Blocker>,
}

/// Works out what `feature`'s first ComfyUI recipe still needs on a computer described by
/// `inventory`. A feature with no ComfyUI recipe gets [`State::NoLocalRoute`] and an empty
/// [`Plan::recipe`]; a built-in route is planned by [`plan_native`].
pub fn plan(feature: &Feature, inventory: &Inventory) -> Plan {
    match feature.comfyui_recipes().next() {
        Some(recipe) => plan_recipe(feature, recipe, inventory),
        None => empty(feature, String::new()),
    }
}

/// Works out what every one of `feature`'s ComfyUI recipes still needs, one [`Plan`] each, in
/// catalogue order. Each names its recipe in [`Plan::recipe`], so several models for one tool
/// can be shown and installed side by side without one standing in for another.
pub fn plans(feature: &Feature, inventory: &Inventory) -> Vec<Plan> {
    feature
        .comfyui_recipes()
        .map(|recipe| plan_recipe(feature, recipe, inventory))
        .collect()
}

/// A plan that has found nothing yet, about `recipe` of `feature`.
fn empty(feature: &Feature, recipe: String) -> Plan {
    Plan {
        feature: feature.id.clone(),
        recipe,
        state: State::NoLocalRoute,
        missing_files: Vec::new(),
        missing_packs: Vec::new(),
        download_bytes: 0,
        licences: Vec::new(),
        blockers: Vec::new(),
    }
}

/// Compares one recipe of `feature` with `inventory`.
fn plan_recipe(feature: &Feature, recipe: &LocalRecipe, inventory: &Inventory) -> Plan {
    let mut out = empty(feature, recipe.workflow.clone());
    if !inventory.comfyui_found {
        out.state = State::NoRuntime;
        return out;
    }
    fill(&mut out, recipe, inventory);
    out.state = if !out.blockers.is_empty() {
        State::Blocked
    } else if out.missing_files.is_empty() && out.missing_packs.is_empty() {
        State::Ready
    } else {
        State::NeedsInstall
    };
    out
}

/// Compares the files of the built-in runtime `provider` for `feature` with `inventory`. Unlike
/// [`plan`], it needs neither ComfyUI nor its node packs: the runtime reads its own files, which
/// are a different layout from the ComfyUI checkpoint. The route comes from that engine's recipe
/// ([`Feature::engine_recipe`]). A feature with no usable recipe for `provider`, or whose route
/// names no files of its own, gets [`State::NoLocalRoute`].
pub fn plan_native(feature: &Feature, provider: &str, inventory: &Inventory) -> Plan {
    let recipe = feature.engine_recipe(provider);
    let mut out = empty(
        feature,
        recipe.map(|r| r.workflow.clone()).unwrap_or_default(),
    );
    let Some(route) = recipe.and_then(|r| {
        r.native
            .iter()
            .find(|n| n.provider == provider && !n.models.is_empty())
    }) else {
        return out;
    };
    fill_files(&mut out, &route.models, inventory);
    check_disk(&mut out, inventory);
    out.state = if !out.blockers.is_empty() {
        State::Blocked
    } else if out.missing_files.is_empty() {
        State::Ready
    } else {
        State::NeedsInstall
    };
    out
}

/// Adds what `recipe` is missing on `inventory` to `out`, and anything that blocks it.
fn fill(out: &mut Plan, recipe: &LocalRecipe, inventory: &Inventory) {
    fill_files(out, &recipe.models, inventory);
    out.missing_packs = recipe
        .packs
        .iter()
        .filter(|p| !inventory.has_pack(p))
        .cloned()
        .collect();
    if let (Some(needed), Some(have)) = (recipe.min_vram_mb, inventory.vram_mb)
        && have < needed
    {
        out.blockers.push(Blocker::NotEnoughVram {
            needed_mb: needed,
            have_mb: have,
        });
    }
    check_disk(out, inventory);
}

/// Adds every file of `models` that `inventory` lacks to `out`, with its bytes and any licence
/// the person must accept.
fn fill_files(out: &mut Plan, models: &[ModelFile], inventory: &Inventory) {
    for model in models {
        if inventory.find(model).is_none() {
            out.download_bytes = out.download_bytes.saturating_add(model.size_bytes);
            if model.licence.accept_required && !out.licences.contains(&model.licence) {
                out.licences.push(model.licence.clone());
            }
            out.missing_files.push(model.clone());
        }
    }
}

/// Blocks `out` when the download is larger than the free disk space `inventory` reports.
fn check_disk(out: &mut Plan, inventory: &Inventory) {
    if let Some(free) = inventory.free_disk_bytes
        && out.download_bytes > free
    {
        out.blockers.push(Blocker::NotEnoughDisk {
            needed_bytes: out.download_bytes,
            free_bytes: free,
        });
    }
}
