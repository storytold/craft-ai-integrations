//! How a feature can run on this computer: which model files and node packs it needs.
//!
//! A recipe is data only. It names files by folder and file name, with the exact size and SHA-256
//! the installer checks after a download. Nothing here touches the disk or the network.

use serde::{Deserialize, Serialize};

/// Where a model file comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// A file in a Hugging Face repository, pinned to a revision when one is known.
    HuggingFace {
        repo: String,
        file: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revision: Option<String>,
    },
    /// A direct https address.
    Url { url: String },
}

impl Source {
    /// The https address of the file.
    pub fn url(&self) -> String {
        match self {
            Self::Url { url } => url.clone(),
            Self::HuggingFace {
                repo,
                file,
                revision,
            } => format!(
                "https://huggingface.co/{repo}/resolve/{}/{file}",
                revision.as_deref().unwrap_or("main")
            ),
        }
    }
}

/// The terms a model is distributed under, as far as we know them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Licence {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The user must accept before the file is downloaded.
    pub accept_required: bool,
    /// Anything the user should know that the name does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A part of the workflow that the person should know about, with the terms its publisher states.
///
/// This is disclosure only. The text repeats what the publisher says on its own page; nobody here
/// has checked it, it is not legal advice, and it carries no warranty or indemnity. Whether a part
/// suits the person's use is their decision, so a component never blocks a download or a run. A
/// part that does need agreement before its download is a [`ModelFile`] with
/// [`Licence::accept_required`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Component {
    /// What it is called, for people: "Florence-2 detector".
    pub label: String,
    /// What it does in the workflow, in a few words.
    pub role: String,
    /// The terms the publisher declares, with a link to where it says so.
    pub licence: Licence,
}

/// One model file a recipe needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelFile {
    /// Folder under the ComfyUI `models` directory, such as `sam3` or `diffusion_models`.
    pub folder: String,
    pub filename: String,
    pub size_bytes: u64,
    /// Lower-case hex SHA-256 of the whole file.
    pub sha256: String,
    pub source: Source,
    pub licence: Licence,
}

/// A ComfyUI custom node pack the workflow needs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodePack {
    pub label: String,
    pub repo_url: String,
    /// Folder names the pack is known to install as under `custom_nodes`, lower case.
    pub folder_names: Vec<String>,
}

/// The same model run by a built-in runtime instead of ComfyUI. The launcher finds or downloads
/// the runtime's files (its own, see [`NativeRoute::models`]) and records the weights file on the
/// connection; the engine reads it from there. A recipe may
/// list several runtimes (Candle, Burn); each is its own connection and its own choice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeRoute {
    /// Adapter name on the connection, such as `candle` or `burn`.
    pub provider: String,
    /// Model id on that connection.
    pub model: String,
    /// What the person sees next to the ComfyUI route.
    pub label: String,
    /// The files this runtime reads, when they are not the recipe's own. A runtime ported from
    /// another framework usually wants that framework's file layout, which is a different file
    /// from the ComfyUI one. The first `.safetensors` or `.onnx` file here is the weights file
    /// recorded on the connection; the rest sit beside it. Empty means the recipe's own files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelFile>,
    /// One plain sentence on what this route cannot do, shown beside it in setup and in the picker
    /// (a route that selects by clicking says so). `None` when there is nothing to add.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Measured quick enough on the computer's main chip, so engine advice does not call it slow
    /// for lack of a graphics card. A curated statement, stated with its evidence in
    /// `docs/STATUS.md`; false means nothing was measured.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fast_on_processor: bool,
}

impl NativeRoute {
    /// The weights file this runtime is given: the first `.safetensors` or `.onnx` file of its
    /// own files.
    pub fn weights(&self) -> Option<&ModelFile> {
        self.models.iter().find(|m| {
            let name = m.filename.to_ascii_lowercase();
            name.ends_with(".safetensors") || name.ends_with(".onnx")
        })
    }

    /// Whether any of this runtime's files needs the person's agreement before its download
    /// ([`Licence::accept_required`]). When none does, a download needs no tick box.
    pub fn needs_agreement(&self) -> bool {
        self.models.iter().any(|m| m.licence.accept_required)
    }

    /// Bytes to download for this runtime's own files.
    pub fn total_bytes(&self) -> u64 {
        self.models
            .iter()
            .fold(0u64, |sum, m| sum.saturating_add(m.size_bytes))
    }
}

/// Running a feature on this computer: through ComfyUI, through the built-in runtimes in
/// [`LocalRecipe::native`], or both.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalRecipe {
    /// What the model is called, for people: "SAM 3".
    pub label: String,
    /// Apps can run it today. False when the files can be downloaded but no workflow runs them yet.
    #[serde(default = "yes")]
    pub usable: bool,
    /// Id of the workflow template the adapter runs. For a recipe ComfyUI does not run, only the
    /// recipe's id.
    pub workflow: String,
    /// ComfyUI runs this recipe. False for a model only the built-in runtimes in
    /// [`LocalRecipe::native`] run: it is then never a ComfyUI setup option, plan or connection,
    /// and its `models` and `packs` are empty.
    #[serde(default = "yes")]
    pub comfyui: bool,
    pub models: Vec<ModelFile>,
    pub packs: Vec<NodePack>,
    /// Graphics memory the workflow needs, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_vram_mb: Option<u32>,
    /// Built-in runtimes that read the same weights file, where an app has one. The first is the
    /// one [`ToolDesk::enable_native`] registers; none means the tool has no built-in runtime.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native: Vec<NativeRoute>,
    /// Everything else the workflow runs, with the terms each publisher declares, shown before
    /// the person chooses to use the model. Empty when nobody has reviewed the parts yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<Component>,
}

fn yes() -> bool {
    true
}

impl LocalRecipe {
    pub fn total_bytes(&self) -> u64 {
        self.models
            .iter()
            .fold(0u64, |sum, m| sum.saturating_add(m.size_bytes))
    }
}

/// A hosted way to get the same result when this computer cannot run the models.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudOption {
    /// The connection preset key (see `craft_ai_core::PRESETS`) that adds it.
    pub preset: String,
    pub label: String,
    /// What the user should know: billing, and whether the route has been tried.
    pub note: String,
}
