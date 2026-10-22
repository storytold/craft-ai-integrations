//! Cache lifecycle. Conversion is injected so its filesystem rules need no real model in tests.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use craft_ai_core::{AiError, ErrorKind};

/// Whether a completed request prepared weights or reused a checked saved copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preparation {
    Converted,
    Reused,
}

/// Checked local weights. This does not register a connection or change any binding.
#[derive(Debug)]
pub struct PreparedWeights {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub preparation: Preparation,
}

/// Expected original bytes for one component, frozen before setup starts.
#[derive(Clone)]
pub(crate) struct SourceFile {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// One supported architecture and converter revision, not an arbitrary model name.
pub(crate) struct Bundle {
    pub revision: String,
    pub files: [SourceFile; 2],
}

/// Converts the two checked original files into weights inside the caller-owned work folder.
pub(crate) trait Converter {
    /// Writes both weight components to the private conversion directory, or reports failure.
    fn convert(&self, source: &Path, output: &Path) -> Result<(), AiError>;
}

/// Saved conversion identity and output checksums. JSON never supplies filesystem paths.
#[derive(Serialize, Deserialize)]
struct Manifest {
    key: String,
    hashes: [String; 2],
}

/// Distinguishes the original checkpoint and converter revision.
fn cache_key(bundle: &Bundle) -> String {
    let mut digest = Sha256::new();
    digest.update(bundle.revision.as_bytes());
    for file in &bundle.files {
        digest.update([0]);
        digest.update(file.name.as_bytes());
        digest.update(file.size.to_le_bytes());
        digest.update(file.sha256.as_bytes());
    }
    hex(&digest.finalize())
}

/// Encodes digest bytes independently of sha2 formatting traits.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Safe local error without paths reaching the screen.
fn file_error(_: impl std::fmt::Display) -> AiError {
    AiError::unsupported(
        "The saved model files could not be read or written. Check the model folder and try again.",
    )
}

/// What a damaged saved copy is answered with. It is public so a host can recognise it and say where
/// the copy lives; retrying cannot help, so this never says "try again".
pub const DAMAGED_COPY: &str = "The saved copy of this model on this computer is damaged, so it was left as it is. Remove that saved copy, then prepare the model again.";

/// The answer for a saved copy that exists but cannot be used (wrong kind of entry, unreadable or
/// oversized manifest, another conversion's manifest, a missing or changed weight file). Nothing is deleted or
/// repaired for the person: a derived copy is still theirs to remove.
fn damaged(_: impl std::fmt::Display) -> AiError {
    AiError::unsupported(DAMAGED_COPY)
}

/// Hashes a file with bounded working memory.
fn hash_file(path: &Path) -> Result<String, AiError> {
    let metadata = fs::symlink_metadata(path).map_err(file_error)?;
    if !metadata.file_type().is_file() || metadata.len() > 512 * 1024 * 1024 {
        return Err(file_error("not a bounded plain file"));
    }
    let mut file = File::open(path).map_err(file_error)?;
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut bytes).map_err(file_error)?;
        if count == 0 {
            return Ok(hex(&hash.finalize()));
        }
        hash.update(&bytes[..count]);
    }
}

/// Resolves a not-yet-created destination through its existing parent without writing anything.
fn destination(path: &Path) -> Result<PathBuf, AiError> {
    let absolute = std::path::absolute(path).map_err(file_error)?;
    let path = absolute.as_path();
    if path
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(file_error("parent traversal in destination"));
    }
    for parent in path.ancestors().take(256) {
        if let Ok(base) = fs::canonicalize(parent) {
            return Ok(base.join(path.strip_prefix(parent).map_err(file_error)?));
        }
    }
    Err(file_error("destination has no existing parent"))
}

/// Cancel is checked before any write and again before publishing a complete conversion.
pub(super) fn check_cancelled(cancelled: &AtomicBool) -> Result<(), AiError> {
    if cancelled.load(Ordering::Acquire) {
        return Err(AiError::new(
            ErrorKind::Cancelled,
            "Model preparation was cancelled.",
        ));
    }
    Ok(())
}

/// Fixed component paths from a complete cache directory.
fn prepared(dir: &Path, preparation: Preparation) -> PreparedWeights {
    PreparedWeights {
        encoder: dir.join("encoder.bpk"),
        decoder: dir.join("decoder.bpk"),
        preparation,
    }
}

/// Prepares a supported bundle without touching the original files or shared settings.
pub(crate) fn prepare(
    bundle: &Bundle,
    source: &Path,
    cache: &Path,
    cancelled: &AtomicBool,
    converter: &dyn Converter,
) -> Result<PreparedWeights, AiError> {
    check_cancelled(cancelled)?;
    if let Ok(original) = fs::canonicalize(source)
        && destination(cache)?.starts_with(original)
    {
        return Err(AiError::unsupported(
            "Choose a separate folder for the saved model copy, outside the original model folder.",
        ));
    }
    fs::create_dir_all(cache).map_err(file_error)?;
    if !fs::symlink_metadata(cache)
        .map_err(file_error)?
        .file_type()
        .is_dir()
    {
        return Err(file_error("cache is not a plain directory"));
    }
    let key = cache_key(bundle);
    let final_dir = cache.join(&key);
    let lock_path = cache.join(format!("{key}.lock"));
    if let Ok(metadata) = fs::symlink_metadata(&lock_path)
        && !metadata.file_type().is_file()
    {
        return Err(file_error("lock is not a plain file"));
    }
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(file_error)?;
    lock.try_lock().map_err(|_| {
        AiError::unsupported(
            "Another app is preparing this model. Wait for it to finish, then try again.",
        )
    })?;
    if let Ok(metadata) = fs::symlink_metadata(&final_dir) {
        if !metadata.file_type().is_dir() {
            return Err(damaged("saved copy is not a plain directory"));
        }
        let path = final_dir.join("manifest.json");
        if !fs::symlink_metadata(&path)
            .map_err(damaged)?
            .file_type()
            .is_file()
        {
            return Err(damaged("manifest is not a plain file"));
        }
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(file_error)?
            .take(4097)
            .read_to_end(&mut bytes)
            .map_err(file_error)?;
        if bytes.len() > 4096 {
            return Err(damaged("oversized manifest"));
        }
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(damaged)?;
        if manifest.key == key {
            for (name, expected) in ["encoder.bpk", "decoder.bpk"]
                .into_iter()
                .zip(&manifest.hashes)
            {
                if hash_file(&final_dir.join(name)).map_err(damaged)? != *expected {
                    return Err(damaged("damaged saved weights"));
                }
            }
            check_cancelled(cancelled)?;
            return Ok(prepared(&final_dir, Preparation::Reused));
        }
        return Err(damaged("different conversion"));
    }
    sweep_stale(cache, &key);
    let scratch = |role: &str| {
        tempfile::Builder::new()
            .prefix(&format!("{key}.{role}."))
            .tempdir_in(cache)
            .map_err(file_error)
    };
    let [work_role, conversion_role, output_role] = SCRATCH_ROLES;
    let work = scratch(work_role)?;
    let conversion = scratch(conversion_role)?;
    let output = scratch(output_role)?;
    for file in &bundle.files {
        let snapshot = work.path().join(&file.name);
        let input = File::open(source.join(&file.name)).map_err(file_error)?;
        let mut target = File::create(&snapshot).map_err(file_error)?;
        let copied = std::io::copy(&mut input.take(file.size.saturating_add(1)), &mut target)
            .map_err(file_error)?;
        drop(target);
        if copied != file.size || hash_file(&snapshot)? != file.sha256 {
            return Err(AiError::unsupported(
                "The original model files do not match this option. Check them in Set up AI first.",
            ));
        }
    }
    check_cancelled(cancelled)?;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        converter.convert(work.path(), conversion.path())
    }))
    .map_err(|_| {
        AiError::unsupported(
            "The model could not be prepared. Try again or choose another option in Set up AI.",
        )
    })??;
    check_cancelled(cancelled)?;
    let mut hashes = [String::new(), String::new()];
    for (name, hash) in ["encoder.bpk", "decoder.bpk"].into_iter().zip(&mut hashes) {
        let converted = conversion.path().join(name);
        let size = fs::metadata(&converted).map_err(file_error)?.len();
        if size == 0 || size > 512 * 1024 * 1024 {
            return Err(file_error("invalid converted file size"));
        }
        fs::rename(&converted, output.path().join(name)).map_err(file_error)?;
        *hash = hash_file(&output.path().join(name))?;
        fs::OpenOptions::new()
            .write(true)
            .open(output.path().join(name))
            .map_err(file_error)?
            .sync_all()
            .map_err(file_error)?;
    }
    let manifest = serde_json::to_vec(&Manifest { key, hashes }).map_err(file_error)?;
    let mut file = File::create(output.path().join("manifest.json")).map_err(file_error)?;
    file.write_all(&manifest).map_err(file_error)?;
    file.sync_all().map_err(file_error)?;
    drop(file);
    check_cancelled(cancelled)?;
    fs::rename(output.path(), &final_dir).map_err(file_error)?;
    Ok(prepared(&final_dir, Preparation::Converted))
}

/// The roles of the scratch folders one preparation makes: the copy of the originals, the
/// converter's output and the folder that becomes the published copy.
const SCRATCH_ROLES: [&str; 3] = ["work", "conv", "out"];

/// Removes the scratch folders a killed run left for this cache key. The caller holds this key's
/// lock, so no live run owns them. Only plain folders named `<key>.<role>.` followed by anything,
/// for the roles above, are removed: another key's folders, a damaged copy renamed to `<key>.backup`,
/// the lock file and any link are left alone.
fn sweep_stale(cache: &Path, key: &str) {
    let prefixes = SCRATCH_ROLES.map(|role| format!("{key}.{role}."));
    let Ok(entries) = fs::read_dir(cache) else {
        return;
    };
    for entry in entries.flatten() {
        let ours = entry
            .file_name()
            .to_str()
            .is_some_and(|n| prefixes.iter().any(|p| n.starts_with(p.as_str())));
        if ours && entry.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests;
