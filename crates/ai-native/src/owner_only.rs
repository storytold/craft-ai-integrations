//! Owner-only access for the folders and files that hold a person's AI settings, history and
//! secrets.
//!
//! Unix does this with mode `0o600` when a file is created. Windows files inherit their folder's
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
//! The Win32 calls live in [`win_acl`], the workspace's one named `unsafe` exception.

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

/// Creates `dir` and any missing parents. On Windows each folder it creates is owner-only from
/// the moment it exists, and passes that on to what is created inside it. Folders that already
/// existed are left alone.
pub(crate) fn create_dir_all(dir: &Path) -> io::Result<()> {
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
    #[cfg(not(windows))]
    {
        std::fs::create_dir_all(dir)
    }
}

#[cfg(test)]
#[cfg(windows)]
#[path = "owner_only/tests.rs"]
mod tests;
