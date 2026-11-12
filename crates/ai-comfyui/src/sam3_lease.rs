//! Exclusive, bounded ownership of a slot in the server's SAM 3 hover cache.
//!
//! The installed interactive node keeps one cached image per graph node id in a plain dictionary:
//! no eviction, no owner check, and the hover route trusts whatever id it is given. A fixed id
//! shared by every session lets one image silently replace another's; a fresh id per session
//! never frees server memory. So each prepared image holds one of a few fixed ids ("slots"),
//! guarded by an operating-system file lock. The lock works across processes (several Craft
//! apps share one ComfyUI) and is released if the holder crashes, so a dead app cannot leak a
//! slot. Nothing here can free the server's cached tensors; the bound on slots is the bound on
//! what this client can leave behind there.
//!
//! The lock folder belongs to the person running the app. It sits in their AI folder, never in a
//! shared temp folder another account could create first, and is created private. On Unix a
//! folder this user does not own, that others can write, or that is a link is refused; on Windows
//! the same goes for a folder whose access list lets another account in. Lock files are never
//! opened through a link. Different accounts sharing one ComfyUI therefore do not
//! coordinate slots: that is the trust boundary, not a gap.
use craft_ai_core::{AiError, ErrorKind};
use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

/// The lock folder's name inside the person's AI folder.
const LOCK_DIR: &str = "sam3-locks";

/// Prepared images one server will hold for Craft clients at once.
pub(crate) const SLOTS: usize = 4;

/// Holding the open, locked file is holding the slot; dropping it releases the slot.
pub(crate) struct SlotLease {
    _file: File,
    slot: usize,
}

impl SlotLease {
    /// Lock the lowest free slot for `server` (a short name unique per server address).
    pub(crate) fn acquire(dir: &Path, server: &str) -> Result<Self, AiError> {
        private_dir(dir).map_err(|_| failed("could not create the SAM 3 lock folder"))?;
        for slot in 0..SLOTS {
            let file = open_lock(&dir.join(format!("slot-{server}-{slot}.lock")))
                .map_err(|_| failed("could not open a SAM 3 lock file"))?;
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file, slot }),
                Err(TryLockError::WouldBlock) => continue,
                Err(TryLockError::Error(_)) => {
                    return Err(failed("could not take a SAM 3 lock"));
                }
            }
        }
        Err(AiError::new(
            ErrorKind::RateLimited,
            "All SAM 3 image slots on this server are in use by other Craft apps or documents. \
             Close Object Selection in one of them and try again",
        ))
    }

    pub(crate) fn slot(&self) -> usize {
        self.slot
    }

    /// The graph node id that names this slot on the server.
    pub(crate) fn node_id(&self) -> String {
        format!("craft_ai_sam3_{}", self.slot)
    }
}

/// Where sessions keep their slot locks unless told otherwise: the person's AI folder, so every
/// Craft app this user runs agrees on it and no other account can create it first.
pub(crate) fn default_dir() -> PathBuf {
    #[cfg(not(target_arch = "wasm32"))]
    let ai_dir = craft_ai_native::paths::ai_dir();
    #[cfg(target_arch = "wasm32")]
    let ai_dir = None;
    choose_dir(ai_dir, std::env::temp_dir())
}

/// The AI folder when there is one. Otherwise a temp folder named for this user on Unix, where the
/// temp folder is usually shared; Windows and macOS give each user their own temp folder.
fn choose_dir(ai_dir: Option<PathBuf>, temp: PathBuf) -> PathBuf {
    match ai_dir {
        Some(dir) => dir.join(LOCK_DIR),
        #[cfg(unix)]
        None => temp.join(format!(
            "craft-ai-sam3-{}",
            rustix::process::geteuid().as_raw()
        )),
        #[cfg(not(unix))]
        None => temp.join("craft-ai-sam3"),
    }
}

/// Creates the lock folder private to this user, and on Unix refuses one that is not.
fn private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    craft_ai_native::owner_only::create_dir_all(dir)?;
    #[cfg(target_arch = "wasm32")]
    std::fs::create_dir_all(dir)?;
    // A folder that already existed is left as it was by the call above, so it is checked here.
    #[cfg(windows)]
    craft_ai_native::owner_only::check_private_dir(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // The folder itself, not what a link there points to.
        let meta = std::fs::symlink_metadata(dir)?;
        let mine = meta.uid() == rustix::process::geteuid().as_raw();
        if !meta.is_dir() || !mine || meta.mode() & 0o022 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the SAM 3 lock folder is not private to this user",
            ));
        }
    }
    Ok(())
}

/// Opens (creating if needed) one slot's lock file, never through a link.
fn open_lock(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT: a link at the name is opened as itself, not followed.
        options.custom_flags(0x0020_0000);
    }
    options.open(path)
}

fn failed(message: &str) -> AiError {
    AiError::new(ErrorKind::Internal, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_folder_is_in_the_ai_folder() {
        let ai = PathBuf::from("ai");
        assert_eq!(
            choose_dir(Some(ai.clone()), PathBuf::from("tmp")),
            ai.join(LOCK_DIR)
        );
    }

    /// Without an AI folder the temp fallback is never the old fixed name another account on a
    /// shared temp folder could create first.
    #[cfg(unix)]
    #[test]
    fn the_fallback_folder_is_named_for_this_user() {
        let dir = choose_dir(None, PathBuf::from("/tmp"));
        let uid = rustix::process::geteuid().as_raw();
        assert_eq!(dir, PathBuf::from(format!("/tmp/craft-ai-sam3-{uid}")));
    }
}
