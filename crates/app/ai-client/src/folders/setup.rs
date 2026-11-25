//! Immutable, reviewable built-in model setup. No secret is stored in the plan.
use crate::{AiClient, FolderDesk, NativeInstall};
use craft_ai_core::AiError;
use craft_ai_features::{Feature, NativeRoute};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
/// A reviewable plan to set up one built-in runtime's files: what it reads, where a download would
/// go, where complete files already are, and how many bytes are still needed. Built once and not
/// changed, so what the person approved is what runs.
pub struct NativeSetupPlan {
    feature: Feature,
    route: NativeRoute,
    destination: Option<PathBuf>,
    existing: Option<PathBuf>,
    download_bytes: u64,
    available: bool,
}

/// Constructed only after all of the selected model's files pass their checks.
#[derive(Clone, Debug, PartialEq, Eq)]
/// The files of one runtime, every one hashed and matching. Only a successful check or download
/// can make one; it names the weights file but does not bind it to any tool.
pub struct CheckedNative {
    feature: Feature,
    provider: String,
    weights: PathBuf,
}
impl CheckedNative {
    /// The weights file the runtime would open.
    pub fn weights(&self) -> &Path {
        &self.weights
    }
    /// The tool the files were checked for.
    pub fn feature(&self) -> &Feature {
        &self.feature
    }
    /// The runtime the files were checked for.
    pub fn provider(&self) -> &str {
        &self.provider
    }
}
impl NativeSetupPlan {
    /// Hash existing files without downloading or writing a binding.
    pub fn check_existing(
        &self,
        cancel: &std::sync::atomic::AtomicBool,
        progress: &mut dyn FnMut(usize, craft_ai_comfy_env::Progress),
    ) -> Result<CheckedNative, AiError> {
        if !self.available {
            return Err(AiError::unsupported("this app cannot use this option yet"));
        }
        let directory = self
            .existing
            .as_deref()
            .ok_or_else(|| AiError::invalid("the model files have not been found"))?;
        let weights = super::native::verify_directory(&self.route, directory, cancel, progress)?;
        Ok(CheckedNative {
            feature: self.feature.clone(),
            provider: self.route.provider.clone(),
            weights,
        })
    }

    /// Download only into a newly made directory. The directory creation is atomic and refuses
    /// an existing directory, so recovery never reuses or replaces a rejected model copy.
    pub fn download_fresh(&self, request: NativeInstall<'_>) -> Result<CheckedNative, AiError> {
        if !self.available {
            return Err(AiError::unsupported("this app cannot use this option yet"));
        }
        if self.route.weights().is_none() {
            return Err(AiError::invalid("that model has no main file to open"));
        }
        if request.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(AiError::new(
                craft_ai_core::ErrorKind::Cancelled,
                "download cancelled",
            ));
        }
        if self.needs_agreement() && !request.licences_accepted {
            return Err(AiError::invalid(
                "accept the model licence before downloading",
            ));
        }
        let destination = self
            .destination()
            .ok_or_else(|| AiError::invalid("choose a new model folder first"))?;
        std::fs::create_dir(destination).map_err(|error| match error.kind() {
            std::io::ErrorKind::AlreadyExists => {
                AiError::invalid("choose a new model folder: this model directory already exists")
            }
            std::io::ErrorKind::NotFound => {
                AiError::invalid("the chosen model folder is missing; choose a new one")
            }
            std::io::ErrorKind::PermissionDenied => {
                AiError::invalid("cannot write to this model folder; choose another location")
            }
            _ => AiError::invalid(format!("could not create the model folder: {error}")),
        })?;
        self.download(request)
    }

    /// The tool this plan is for.
    pub fn feature(&self) -> &Feature {
        &self.feature
    }
    /// The runtime route, with the files it reads.
    pub fn route(&self) -> &NativeRoute {
        &self.route
    }
    /// Where a download would write, or `None` when no default model folder is chosen.
    pub fn destination(&self) -> Option<&Path> {
        self.destination.as_deref()
    }
    /// The directory that already holds every file by name and size, or `None`. Not yet hashed.
    pub fn existing_directory(&self) -> Option<&Path> {
        self.existing.as_deref()
    }
    /// Bytes a download would fetch, counting only files already present at the destination as reusable.
    pub fn download_bytes(&self) -> u64 {
        self.download_bytes
    }
    /// Whether the person must agree to a licence before the download starts. False for files whose
    /// terms need no agreement (SAM 2.1), so the setup screen asks for nothing it does not need.
    pub fn needs_agreement(&self) -> bool {
        self.route.needs_agreement()
    }
    /// Whether this app has an adapter for the runtime. Without one nothing here can be used.
    pub fn available(&self) -> bool {
        self.available
    }
    /// Whether `other` is a plan for what this one asks the person to approve: the same tool,
    /// runtime, destination and availability. The bytes still needed and the directory where
    /// complete files were found change as a download makes progress, so they do not count.
    pub fn same_target(&self, other: &Self) -> bool {
        self.feature == other.feature
            && self.route == other.route
            && self.destination == other.destination
            && self.available == other.available
    }
    /// On a worker, download into the exact destination shown in this plan. No binding is written.
    pub fn download(&self, request: NativeInstall<'_>) -> Result<CheckedNative, AiError> {
        if !self.available {
            return Err(AiError::unsupported("this app cannot use this option yet"));
        }
        let directory = self
            .destination
            .as_deref()
            .ok_or_else(|| AiError::invalid("choose a model folder for downloads first"))?;
        let weights = self
            .route
            .weights()
            .ok_or_else(|| AiError::invalid("that model has no main file to open"))?;
        super::native::install_into(&self.route, directory, request)?;
        Ok(CheckedNative {
            feature: self.feature.clone(),
            provider: self.route.provider.clone(),
            weights: directory.join(&weights.filename),
        })
    }
}
impl FolderDesk {
    /// Build on a worker: folder discovery reads disk, but does not hash, download or write.
    pub fn prepare_native_setup(
        &self,
        feature: &Feature,
        provider: &str,
        client: &AiClient,
    ) -> Result<NativeSetupPlan, AiError> {
        let route = super::native::native_route(feature, provider)?;
        let found = self.native_files(feature, provider)?;
        let complete = craft_ai_features::plan_native(feature, provider, &found.inventory)
            .missing_files
            .is_empty();
        let existing = found.directory.filter(|_| complete);
        let destination = self.native_download_dir(feature, provider).ok();
        // Files elsewhere are not reused by a download into this destination.
        let download_bytes = route
            .models
            .iter()
            .filter(|file| {
                !destination
                    .as_ref()
                    .and_then(|dir| std::fs::metadata(dir.join(&file.filename)).ok())
                    .is_some_and(|metadata| metadata.is_file() && metadata.len() == file.size_bytes)
            })
            .fold(0u64, |total, file| total.saturating_add(file.size_bytes));
        Ok(NativeSetupPlan {
            feature: feature.clone(),
            route: route.clone(),
            destination,
            existing,
            download_bytes,
            available: client.has_adapter(provider),
        })
    }
}
