//! Explicit local setup probe. Never downloads, changes a binding or contacts a server.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

/// Runs one approved preparation and reports its actual checked output paths.
fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [source, cache] = args.as_slice() else {
        eprintln!("usage: prepare_sam21 <original-model-folder> <shared-model-cache-folder>");
        return ExitCode::from(2);
    };
    let start = Instant::now();
    match craft_ai_burn_setup::prepare_sam21(
        &PathBuf::from(source),
        &PathBuf::from(cache),
        &AtomicBool::new(false),
    ) {
        Ok(weights) => {
            println!(
                "{}",
                serde_json::json!({
                    "preparation": format!("{:?}", weights.preparation),
                    "encoder": weights.encoder,
                    "decoder": weights.decoder,
                    "elapsed_ms": start.elapsed().as_millis(),
                })
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", error.message);
            ExitCode::FAILURE
        }
    }
}
