//! The host step for Burn: turn the two downloaded publisher files into the saved copy this engine
//! reads, once, on this computer.
//!
//! The person chose Burn and the files were downloaded by the normal setup, so loading from the
//! downloaded `encoder.onnx` is the moment to prepare. The saved copy goes in a folder beside the
//! model folder (shared by every app that uses the same model folder, and never inside it).
//! `craft-ai-burn-setup` checks the exact source bytes, reuses a checked saved copy, and publishes
//! a whole folder or nothing. The converter cannot be interrupted part-way and this call has no
//! Cancel handle, so it runs to the end; the first run takes seconds, later runs only check hashes.

use std::path::{Path, PathBuf};

use craft_ai_core::AiError;

/// The folder name for saved copies, beside the model folder.
#[cfg(feature = "prepare")]
const PREPARED_DIR: &str = "burn-prepared";

/// Said when the engine cannot prepare, so the person knows the next step.
const OPEN_SETUP: &str =
    "This option has to prepare SAM 2.1 on this computer first. Open Set up AI to do it.";

/// Prepares (or reuses) the saved copy for the `encoder.onnx` given and returns the folder that
/// holds `encoder.bpk` and `decoder.bpk`.
#[cfg(feature = "prepare")]
pub(super) fn prepared_folder(encoder: &Path) -> Result<PathBuf, AiError> {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::AtomicBool;

    let source = encoder
        .parent()
        .filter(|d| d.join("decoder.onnx").is_file());
    let (Some(source), true) = (source, encoder.is_file()) else {
        return Err(AiError::unsupported(
            "SAM 2.1 needs encoder.onnx and decoder.onnx together in one folder. Open Set up AI to download them.",
        ));
    };
    let Some(cache) = source.parent().map(|p| p.join(PREPARED_DIR)) else {
        return Err(AiError::unsupported(OPEN_SETUP));
    };
    let never = AtomicBool::new(false);
    // The converter is third-party code that reports a bad graph by panicking.
    let prepared = catch_unwind(AssertUnwindSafe(|| {
        craft_ai_burn_setup::prepare_sam21(source, &cache, &never)
    }))
    .map_err(|_| {
        AiError::unsupported(
            "SAM 2.1 could not be prepared from these files. Open Set up AI to download them again.",
        )
    })?
    .map_err(explain)?;
    prepared
        .encoder
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| AiError::unsupported(OPEN_SETUP))
}

/// Says where a damaged saved copy lives. Trying again cannot help (the copy is refused, not
/// repaired), so the person is told to delete the folder; any other failure passes through.
#[cfg(feature = "prepare")]
pub(super) fn explain(error: AiError) -> AiError {
    if error.message == craft_ai_burn_setup::DAMAGED_COPY {
        return AiError::unsupported(format!(
            "The saved copy of SAM 2.1 is damaged, so it was left as it is. Delete the {PREPARED_DIR} folder beside the model folder, then open this option again."
        ));
    }
    error
}

/// Without the converter built in, a downloaded file is answered with what to do.
#[cfg(not(feature = "prepare"))]
pub(super) fn prepared_folder(_encoder: &Path) -> Result<PathBuf, AiError> {
    Err(AiError::unsupported(OPEN_SETUP))
}
