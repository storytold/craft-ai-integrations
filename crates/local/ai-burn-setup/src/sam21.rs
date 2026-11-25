//! The exact supported SAM 2.1 tiny export. A different graph needs a compiled architecture too.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::cache::{self, Bundle, Converter, PreparedWeights, SourceFile};
use craft_ai_core::AiError;

/// Bump when the compiled architecture, tensor mapping or converter changes.
const REVISION: &str = "sam21-tiny-burn-0.22.1-onnx-0.22.0-v1";

/// Only these reviewed source bytes can be converted for the shipped architecture.
fn bundle() -> Bundle {
    let source = |name: &str, size, sha256: &str| SourceFile {
        name: name.into(),
        size,
        sha256: sha256.into(),
    };
    Bundle {
        revision: REVISION.into(),
        files: [
            source(
                "encoder.onnx",
                109_473_919,
                "4fda6e68561e27808cedffa8689749a622bba5f11ba66f4ca4e3a8a9f6360db5",
            ),
            source(
                "decoder.onnx",
                20_655_795,
                "0ae944e08e55814c19afa4d143ef07b1a3dbad2decd77c15ec3ab220b9f57247",
            ),
        ],
    }
}

/// Prepares SAM 2.1 tiny weights once, or checks and reuses its saved conversion.
///
/// Call on a worker only after the person explicitly chooses this engine and approves setup.
/// `source` holds the two publisher ONNX files. `cache` is a host-selected, shared, app-owned
/// model-cache folder, never the source folder. Nothing is downloaded, compiled or registered.
/// The synchronous converter cannot be interrupted mid-component: Cancel discards its result
/// when it returns. This does not make arbitrary ONNX graphs executable in Burn.
pub fn prepare_sam21(
    source: &Path,
    cache: &Path,
    cancelled: &AtomicBool,
) -> Result<PreparedWeights, AiError> {
    cache::prepare(
        &bundle(),
        source,
        cache,
        cancelled,
        &OnnxConverter(cancelled),
    )
}

/// Uses a bundled library, not Cargo, rustc, rustfmt, Python or a separate installed converter.
struct OnnxConverter<'a>(&'a AtomicBool);

impl Converter for OnnxConverter<'_> {
    /// Converts only the already-checked private copy of the known source files.
    fn convert(&self, source: &Path, output: &Path) -> Result<(), AiError> {
        for name in ["encoder.onnx", "decoder.onnx"] {
            cache::check_cancelled(self.0)?;
            let input = source.join(name);
            let input = input.to_str().ok_or_else(|| {
                AiError::unsupported(
                    "Choose a model folder whose name uses standard text characters.",
                )
            })?;
            let output = output.to_str().ok_or_else(|| {
                AiError::unsupported(
                    "Choose a saved-model folder whose name uses standard text characters.",
                )
            })?;
            burn_onnx::ModelGen::new()
                .input(input)
                .out_dir(output)
                .run_from_cli();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Preparation;

    /// The starter download and compiled architecture must agree on the exact original files.
    #[test]
    fn the_supported_source_bytes_match_the_catalogue() {
        let feature = craft_ai_features::builtin()
            .into_iter()
            .find(|f| f.id == "select-subject")
            .unwrap();
        let route = feature
            .engine_recipe("rten")
            .unwrap()
            .native
            .iter()
            .find(|r| r.provider == "rten")
            .unwrap();
        for expected in bundle().files {
            let offered = route
                .models
                .iter()
                .find(|f| f.filename == expected.name)
                .unwrap();
            assert_eq!(offered.sha256, expected.sha256);
            assert_eq!(offered.size_bytes, expected.size);
        }
    }

    /// Real conversion followed by verified reuse, using existing checked model files only.
    #[test]
    #[ignore = "requires CRAFT_TEST_SAM21_ONNX_DIR pointing at the two original files"]
    fn the_real_model_is_prepared_once_and_reused() {
        let source =
            std::env::var_os("CRAFT_TEST_SAM21_ONNX_DIR").expect("set the original file folder");
        let dir = tempfile::tempdir().unwrap();
        let cancelled = AtomicBool::new(false);
        let first = prepare_sam21(Path::new(&source), dir.path(), &cancelled).unwrap();
        assert_eq!(first.preparation, Preparation::Converted);
        assert!(std::fs::metadata(&first.encoder).unwrap().len() > 100_000_000);
        assert!(std::fs::metadata(&first.decoder).unwrap().len() > 10_000_000);
        let second = prepare_sam21(Path::new(&source), dir.path(), &cancelled).unwrap();
        assert_eq!(second.preparation, Preparation::Reused);
        assert_eq!(first.encoder, second.encoder);
    }
}
