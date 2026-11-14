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

/// Refuses a folder that is not private to this user. Windows only: [`create_dir_all`] leaves a
/// folder that already existed alone, so a caller that keeps something in a fixed folder (SAM 3
/// slot locks) checks it before use. A link or junction is refused, and so is a folder owned by
/// another account or whose access list lets any account other than this user, SYSTEM or the
/// Administrators group in. (Unix callers compare the folder's owner and mode instead.)
#[cfg(windows)]
pub fn check_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    // FILE_ATTRIBUTE_REPARSE_POINT: a link or junction, judged by itself rather than its target.
    const REPARSE_POINT: u32 = 0x400;
    let meta = std::fs::symlink_metadata(dir)?;
    let private = meta.is_dir()
        && meta.file_attributes() & REPARSE_POINT == 0
        && sddl_is_private(&win_acl::sddl_of(dir)?, &win_acl::user_sid()?);
    if private {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the folder is not private to this user",
        ))
    }
}

/// Whether an owner-and-access-list SDDL string (`O:…D:…`) keeps out every account but `me`
/// (a SID), SYSTEM and the Administrators group. Allow entries are checked whatever their
/// inheritance flags, because what they pass on reaches the files created inside. Deny entries
/// only take access away. A missing (null) access list lets everyone in.
#[cfg(any(windows, test))]
fn sddl_is_private(sddl: &str, me: &str) -> bool {
    // SDDL writes the machine's built-in Administrator account (RID 500) as `LA`.
    let me_alias = (me.starts_with("S-1-5-21-") && me.ends_with("-500")).then_some("LA");
    // CREATOR OWNER and OWNER RIGHTS name whoever owns the object, checked separately.
    let trusted =
        |who: &str| who == me || Some(who) == me_alias || matches!(who, "SY" | "BA" | "CO" | "OW");
    let Some(rest) = sddl.strip_prefix("O:") else {
        return false;
    };
    let Some(dacl_at) = rest.find("D:") else {
        return false;
    };
    let owner = rest[..dacl_at].split("G:").next().unwrap_or_default();
    if !(owner == me || Some(owner) == me_alias || owner == "BA") {
        return false;
    }
    let dacl = &rest[dacl_at + 2..];
    let dacl = dacl.split("S:").next().unwrap_or_default();
    if dacl.starts_with("NO_ACCESS_CONTROL") {
        return false;
    }
    dacl.split('(').skip(1).all(|ace| {
        let fields: Vec<&str> = ace.trim_end_matches(')').split(';').collect();
        let (Some(kind), Some(who)) = (fields.first(), fields.get(5)) else {
            return false;
        };
        let allows = matches!(*kind, "A" | "OA" | "XA" | "ZA");
        !allows || trusted(who)
    })
}

#[cfg(test)]
mod sddl_tests {
    use super::sddl_is_private;

    const ME: &str = "S-1-5-21-1-2-3-1001";

    #[test]
    fn a_folder_this_crate_creates_is_private() {
        let made = format!("O:{ME}D:P(A;OICI;FA;;;{ME})(A;OICI;FA;;;SY)");
        assert!(sddl_is_private(&made, ME));
    }

    #[test]
    fn the_usual_inherited_list_of_a_profile_folder_is_private() {
        let temp = format!("O:BAD:(A;OICIID;FA;;;SY)(A;OICIID;FA;;;BA)(A;OICIID;FA;;;{ME})");
        assert!(sddl_is_private(&temp, ME));
    }

    #[test]
    fn the_built_in_administrator_is_recognised_by_its_alias() {
        let admin = "S-1-5-21-1-2-3-500";
        assert!(sddl_is_private(
            "O:LAD:P(A;OICI;FA;;;LA)(A;OICI;FA;;;SY)",
            admin
        ));
    }

    #[test]
    fn a_folder_owned_by_another_account_is_refused() {
        let other = format!("O:S-1-5-21-1-2-3-1002D:P(A;OICI;FA;;;{ME})");
        assert!(!sddl_is_private(&other, ME));
    }

    #[test]
    fn any_other_account_let_in_is_refused() {
        for who in ["WD", "AU", "BU", "S-1-5-21-1-2-3-1002"] {
            let shared = format!("O:{ME}D:P(A;OICI;FA;;;{ME})(A;OICIIO;0x1301bf;;;{who})");
            assert!(!sddl_is_private(&shared, ME), "{who}");
        }
    }

    #[test]
    fn deny_entries_and_a_group_part_do_not_matter() {
        let denied = format!("O:{ME}G:SYD:P(D;OICI;FA;;;WD)(A;OICI;FA;;;{ME})");
        assert!(sddl_is_private(&denied, ME));
    }

    #[test]
    fn a_missing_access_list_or_malformed_text_is_refused() {
        assert!(!sddl_is_private(&format!("O:{ME}D:NO_ACCESS_CONTROL"), ME));
        assert!(!sddl_is_private(&format!("O:{ME}"), ME));
        assert!(!sddl_is_private("garbage", ME));
        assert!(!sddl_is_private(&format!("O:{ME}D:P(A;OICI)"), ME));
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
