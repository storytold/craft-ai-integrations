//! Discovery and installation of a built-in runner's own files, separate from ComfyUI.
//! Discovery is read only; downloads require explicit approval and go to the chosen default.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use craft_ai_comfy_env::{
    Basis, Downloader, KnownFile, Progress, ScanError, scan_model_folder, verify_file,
};
use craft_ai_core::model_folders::FolderOrigin;
use craft_ai_core::{AiError, ErrorKind, SecretString};
use craft_ai_features::{Feature, InstalledFile, Inventory, NativeRoute};

use super::FolderDesk;

/// One native model's files, confined to a single directory.
#[derive(Debug)]
pub struct NativeFiles {
    pub directory: Option<PathBuf>,
    pub inventory: Inventory,
}

/// Explicit approval and transfer controls for a native model installation.
pub struct NativeInstall<'a> {
    pub downloader: &'a Downloader,
    pub token: Option<&'a SecretString>,
    pub licences_accepted: bool,
    pub cancel: &'a AtomicBool,
    pub progress: &'a mut dyn FnMut(usize, Progress),
}

impl FolderDesk {
    /// Finds this runner's files in the person's registered folders without moving them. Only
    /// files in the same directory count together; other routes' checkpoints cannot substitute.
    /// The inventory checks names and sizes, not hashes. Verify before loading or reusing files.
    pub fn native_files(&self, feature: &Feature, provider: &str) -> Result<NativeFiles, AiError> {
        let route = native_route(feature, provider)?;
        let known: Vec<KnownFile<'_>> = route
            .models
            .iter()
            .map(|m| KnownFile {
                kind: &m.folder,
                filename: &m.filename,
                size: m.size_bytes,
            })
            .collect();
        let mut candidates: BTreeMap<PathBuf, Vec<InstalledFile>> = BTreeMap::new();
        for root in self.list()? {
            let root = Path::new(&root.path);
            // A folder that has gone (an unplugged drive, a deleted folder) is not a candidate. It
            // must not stop discovery in the folders that remain.
            let scan = match scan_model_folder(root, &known) {
                Ok(scan) => scan,
                Err(ScanError::NotAFolder) => continue,
            };
            for found in scan.files.iter().filter(|f| f.basis == Some(Basis::Recipe)) {
                let path = root.join(&found.rel);
                let Some(dir) = path.parent() else { continue };
                let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                let Some(model) = route
                    .models
                    .iter()
                    .find(|m| m.filename.eq_ignore_ascii_case(name) && m.size_bytes == found.size)
                else {
                    continue;
                };
                let files = candidates.entry(dir.to_owned()).or_default();
                if !files.iter().any(|f| f.filename == model.filename) {
                    files.push(InstalledFile {
                        folder: model.folder.clone(),
                        filename: model.filename.clone(),
                        size_bytes: model.size_bytes,
                    });
                }
            }
        }
        let best = candidates.into_iter().max_by_key(|(_, files)| files.len());
        let (directory, files) = match best {
            Some((directory, files)) => (Some(directory), files),
            None => (None, Vec::new()),
        };
        Ok(NativeFiles {
            directory,
            inventory: Inventory {
                files,
                ..Inventory::default()
            },
        })
    }

    /// Checks all of an existing runner's files before its weights are enabled. Returns the
    /// weights path, not permission to run a job. Does not download or change files.
    pub fn verify_native_files(
        &self,
        feature: &Feature,
        provider: &str,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(usize, Progress),
    ) -> Result<PathBuf, AiError> {
        let route = native_route(feature, provider)?;
        let directory = self
            .native_files(feature, provider)?
            .directory
            .ok_or_else(|| AiError::invalid("the model files have not been found"))?;
        verify_directory(route, &directory, cancel, progress)
    }

    /// Destination for a built-in runner's files. The caller must ask for the required licence
    /// and fetch and hash the files; reading this path never creates it or changes another app's
    /// model folder.
    pub fn native_download_dir(
        &self,
        feature: &Feature,
        provider: &str,
    ) -> Result<PathBuf, AiError> {
        let route = native_route(feature, provider)?;
        let folder = &route.models[0].folder;
        if folder.is_empty()
            || folder == "."
            || folder == ".."
            || folder.contains(['/', '\\', ':'])
            || route.models.iter().any(|m| m.folder != *folder)
        {
            return Err(AiError::invalid(
                "the model files have no safe common folder",
            ));
        }
        let default = self
            .list()?
            .into_iter()
            .find(|f| f.default && f.origin == FolderOrigin::Chosen)
            .ok_or_else(|| AiError::invalid("choose a model folder for downloads first"))?;
        Ok(Path::new(&default.path).join(folder))
    }

    /// Installs all of a runner's declared files into the chosen default folder. The caller
    /// obtains any gated token and explicit licence approval first. The downloader resumes
    /// partial transfers; even a reused same-size file is hashed before it can be used.
    pub fn install_native(
        &self,
        feature: &Feature,
        provider: &str,
        request: NativeInstall<'_>,
    ) -> Result<PathBuf, AiError> {
        let route = native_route(feature, provider)?;
        let destination = self.native_download_dir(feature, provider)?;
        install_into(route, &destination, request)?;
        Ok(destination)
    }
}

/// Hashes every file of `route` in `directory` against its declared size and SHA-256, reporting
/// progress per file, and returns the weights path. Writes nothing.
pub(super) fn verify_directory(
    route: &NativeRoute,
    directory: &Path,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize, Progress),
) -> Result<PathBuf, AiError> {
    let weights = route
        .weights()
        .ok_or_else(|| AiError::invalid("that model has no main file to open"))?;
    for (index, file) in route.models.iter().enumerate() {
        verify_file(&directory.join(&file.filename), file, cancel, &mut |p| {
            progress(index, p)
        })?;
    }
    Ok(directory.join(&weights.filename))
}

/// The destination was already shown and approved. Never look up a new default mid-transfer.
pub(super) fn install_into(
    route: &NativeRoute,
    destination: &Path,
    request: NativeInstall<'_>,
) -> Result<(), AiError> {
    if request.cancel.load(Ordering::Relaxed) {
        return Err(AiError::new(ErrorKind::Cancelled, "download cancelled"));
    }
    if !request.licences_accepted
        && route
            .models
            .iter()
            .any(|model| model.licence.accept_required)
    {
        return Err(AiError::invalid(
            "accept the model licence before downloading",
        ));
    }
    for (index, model) in route.models.iter().enumerate() {
        let mut progress = |p| (request.progress)(index, p);
        let path = request.downloader.fetch(
            model,
            destination,
            request.token,
            request.cancel,
            &mut progress,
        )?;
        verify_file(&path, model, request.cancel, &mut progress)?;
    }
    Ok(())
}

/// The route for `provider` in that engine's recipe ([`Feature::engine_recipe`]), when it
/// names files of its own. Refused when there is none, or when a file name is not a plain name
/// (empty, dotted, or holding a slash or colon).
pub(super) fn native_route<'a>(
    feature: &'a Feature,
    provider: &str,
) -> Result<&'a NativeRoute, AiError> {
    let route = feature
        .engine_recipe(provider)
        .and_then(|recipe| {
            recipe
                .native
                .iter()
                .find(|route| route.provider == provider && !route.models.is_empty())
        })
        .ok_or_else(|| AiError::invalid("that tool has no built-in model with those files"))?;
    if route.models.iter().any(|file| {
        file.filename.is_empty()
            || file.filename.len() > 255
            || file.filename.starts_with('.')
            || file.filename.contains(['/', '\\', ':'])
    }) {
        return Err(AiError::invalid("the model file name is not a plain name"));
    }
    Ok(route)
}
