//! Owner-only access for the folders and files that hold a person's AI settings, history and
//! secrets.
//!
//! Unix does this with mode `0o600` when a file is created, and `0o700` for a folder this crate
//! creates. Windows files inherit their folder's
//! access list instead. Under `%LOCALAPPDATA%` that list is private, but a folder that
//! `CRAFT_AI_DIR` names elsewhere may not be: a new folder under `C:\` lets every signed-in
//! account read and change what is written there. So on Windows:
//!
//! - a private file is created new, with an owner-only access list given in the same call, so
//!   there is no moment when another account could open it;
//! - a folder this crate creates is made the same way, and passes its list to everything created
//!   inside it. Folders that already existed are left alone: they may be shared on purpose.
//!
//! Files are always new and never reused: an existing file keeps whoever made it as its owner.
//!
//! Other Craft crates that keep files in a person's AI folder (SAM 3 slot locks, the ComfyUI
//! extra-paths file) use [`create_dir_all`] and [`write_atomic`] so they get the same rules.
//!
//! The Win32 calls live in `win_acl`, the workspace's one named `unsafe` exception.

use std::fs::File;
use std::io;
use std::path::Path;

#[cfg(windows)]
mod win_acl;

/// Creates a new file only its owner can open, ready for writing. Fails if `path` exists.
pub(crate) fn create_file(path: &Path) -> io::Result<File> {
    #[cfg(windows)]
    {
        win_acl::create_file(path)
    }
    #[cfg(not(windows))]
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path)
    }
}

/// Writes `bytes` to `path` so a reader sees the old file or the new one, never a mix. The file
/// is private to its owner, and a link planted at the temporary name is replaced, never followed.
/// Missing parent folders are created as [`create_dir_all`] does.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    crate::fsutil::atomic_write(path, bytes)
}

/// Creates `dir` and any missing parents. Each folder it creates is owner-only from the moment it
/// exists (mode `0o700` on Unix; on Windows an owner-only access list that is passed on to what
/// is created inside). Folders that already existed are left alone.
pub fn create_dir_all(dir: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        let mut missing = Vec::new();
        let mut cursor = Some(dir);
        while let Some(d) = cursor {
            if d.as_os_str().is_empty() || d.exists() {
                break;
            }
            missing.push(d);
            cursor = d.parent();
        }
        // Outermost first, so each new folder is created inside one that already exists.
        for d in missing.iter().rev() {
            match win_acl::create_dir(d) {
                // Another process made it meanwhile. Files inside are still created owner-only.
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists && d.is_dir() => {}
                other => other?,
            }
        }
        Ok(())
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(any(windows, unix)))]
    {
        std::fs::create_dir_all(dir)
    }
}

#[cfg(test)]
#[cfg(windows)]
#[path = "owner_only/tests.rs"]
mod tests;

#[cfg(test)]
#[cfg(unix)]
#[path = "owner_only/unix_tests.rs"]
mod unix_tests;
