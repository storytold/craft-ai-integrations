//! Owner-only access for the folders and files that hold a person's AI settings, history and
//! secrets.
//!
//! Unix does this with mode `0o600` when a file is created. Windows files inherit their folder's
//! access list instead. Under `%LOCALAPPDATA%` that list is private, but a folder that
//! `CRAFT_AI_DIR` names elsewhere may not be: a new folder under `C:\` lets every signed-in
//! account read and change what is written there. So on Windows:
//!
//! - a private file is created with no sharing (no other handle can open it meanwhile), then
//!   given an owner-only list through its own handle before any byte is written;
//! - a folder this crate creates gets an owner-only list that everything created inside it
//!   inherits. Folders that already existed are left alone: they may be shared on purpose.
//!
//! "Owner" is `OWNER RIGHTS`, whoever owns the object. That is the person only for an object this
//! process created, which is why files are always new (`create_new`) and never reused.
//!
//! The Win32 calls live in [`win_acl`], the workspace's one named `unsafe` exception.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

#[cfg(windows)]
mod win_acl;

/// Creates a new file only its owner can open, ready for writing. Fails if `path` exists.
pub(crate) fn create_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // WRITE_DAC to set the list through this handle; no sharing, so no other process holds
        // the file open from before the list changed.
        options
            .access_mode(win_acl::FILE_GENERIC_WRITE | win_acl::WRITE_DAC)
            .share_mode(0);
    }
    let file = options.open(path)?;
    #[cfg(windows)]
    if let Err(e) = win_acl::protect_file(&file, win_acl::FILE_SDDL) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    Ok(file)
}

/// Creates `dir` and any missing parents, limiting each folder it creates to its owner.
pub(crate) fn create_dir_all(dir: &Path) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut cursor = Some(dir);
    while let Some(d) = cursor {
        if d.as_os_str().is_empty() || d.exists() {
            break;
        }
        missing.push(d);
        cursor = d.parent();
    }
    std::fs::create_dir_all(dir)?;
    // Outermost first, so no new folder is reachable through a new parent still open to others.
    for d in missing.iter().rev() {
        restrict_dir(d)?;
    }
    Ok(())
}

fn restrict_dir(dir: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        win_acl::protect_path(dir, win_acl::DIR_SDDL)
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        Ok(())
    }
}

#[cfg(test)]
#[cfg(windows)]
#[path = "owner_only/tests.rs"]
mod tests;
