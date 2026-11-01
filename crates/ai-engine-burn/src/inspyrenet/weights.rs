//! Filling the network from its checkpoint: the published InSPyReNet Swin-B (Plus Ultra) weights
//! as a `.safetensors` file, read with Burn's PyTorch adapter (PyTorch's `[out, in]` linear
//! weights are transposed as they load). A saved Burn copy (`.bpk`) is read too.

use std::io::Read;
use std::path::Path;

use burn_store::{
    ApplyResult, BurnpackStore, ModuleSnapshot, PyTorchToBurnAdapter, SafetensorsStore,
};
use craft_ai_core::AiError;

use super::net::InSPyReNet;

/// The first bytes of a saved Burn weight file.
const BURNPACK: &[u8; 4] = b"NRUB";

/// Buffers the checkpoint carries that the port computes itself.
fn recomputed(key: &str) -> bool {
    key.ends_with(".relative_position_index") || key.ends_with(".num_batches_tracked")
}

/// Said when a file cannot be used: damaged, cut short, or another model's.
pub(super) fn unreadable() -> AiError {
    AiError::unsupported(
        "This file is not the InSPyReNet checkpoint Craft AI expects. Open Set up AI to download it again.",
    )
}

/// Whether every weight was set, none failed, and nothing in the file was left over apart from the
/// buffers the port recomputes.
pub(super) fn complete(result: &ApplyResult) -> bool {
    result.is_success() && result.missing.is_empty() && result.unused.iter().all(|k| recomputed(k))
}

/// Whether the file starts like a saved Burn weight file.
fn is_burnpack(path: &Path) -> bool {
    let mut head = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && &head == BURNPACK
}

/// Fills `net` from the file at `path`, refusing one that is damaged, leaves any weight unset or
/// holds tensors this network does not have.
pub(super) fn fill(mut net: InSPyReNet, path: &Path) -> Result<InSPyReNet, AiError> {
    let applied = if is_burnpack(path) {
        net.load_from(&mut BurnpackStore::from_file(path))
            .map_err(|_| unreadable())?
    } else {
        let mut store = SafetensorsStore::from_file(path).with_from_adapter(PyTorchToBurnAdapter);
        net.load_from(&mut store).map_err(|_| unreadable())?
    };
    if complete(&applied) {
        Ok(net)
    } else {
        Err(unreadable())
    }
}

/// The network with placeholder weights on `device`, built the way a load builds it.
#[cfg(test)]
pub(super) fn untrained(device: &burn::prelude::Device) -> InSPyReNet {
    InSPyReNet::new(device)
}
