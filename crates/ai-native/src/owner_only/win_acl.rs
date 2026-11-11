//! The workspace's one `unsafe` exception, named in `AGENTS.md` and `CONTRIBUTING.md`: the Win32
//! calls that give a file or folder an owner-only access list. Std has no safe way to set one.
//!
//! Only safe functions leave this module. Each `unsafe` block is a single foreign call whose
//! pointers come from values that outlive it, and `tests/unsafe_exception.rs` fails if `unsafe`
//! is allowed anywhere else.
//!
//! The lists, as SDDL: protected (`P`, nothing inherited from the parent), full control (`FA`)
//! for `OWNER RIGHTS` (`OW`, whoever owns the object) and `SYSTEM` (`SY`). A folder's entries
//! are inherited by the files and folders created inside it (`OICI`).
#![allow(unsafe_code)]

use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree, WIN32_ERROR};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT,
    SetNamedSecurityInfoW, SetSecurityInfo,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, OBJECT_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
};
pub(super) use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_WRITE, WRITE_DAC};

/// Owner-only list for one file.
pub(super) const FILE_SDDL: &str = "D:P(A;;FA;;;OW)(A;;FA;;;SY)";
/// Owner-only list for a folder, passed on to everything created inside it.
pub(super) const DIR_SDDL: &str = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)";

/// Replace the access list, and stop inheriting from the parent.
const INFO: OBJECT_SECURITY_INFORMATION =
    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;

/// A security descriptor the system allocated; freed exactly once, on drop.
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from ConvertStringSecurityDescriptorToSecurityDescriptorW, which
        // allocates with LocalAlloc, and this is the only place it is freed.
        unsafe { LocalFree(self.0) };
    }
}

/// Applies `sddl`'s access list to an open file through its handle, which needs `WRITE_DAC`.
/// Using the handle rather than the path means the list lands on the file this process created,
/// even if the name was swapped since.
pub(super) fn protect_file(file: &File, sddl: &str) -> io::Result<()> {
    let sd = descriptor(sddl)?;
    let acl = dacl(&sd)?;
    // SAFETY: the handle is borrowed from `file`, which is open for the whole call; `acl` points
    // into `sd`, which is alive until the end of this function. Owner, group and SACL are null,
    // which INFO says not to change.
    let code = unsafe {
        SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            INFO,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null(),
        )
    };
    check(code)
}

/// Applies `sddl`'s access list to the file or folder at `path`.
pub(super) fn protect_path(path: &Path, sddl: &str) -> io::Result<()> {
    let name = wide(path.as_os_str())?;
    let sd = descriptor(sddl)?;
    let acl = dacl(&sd)?;
    // SAFETY: `name` is NUL-terminated with no interior NUL and lives past the call; `acl` points
    // into `sd`, alive until the end of this function; null owner, group and SACL are unchanged.
    let code = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            INFO,
            ptr::null_mut(),
            ptr::null_mut(),
            acl,
            ptr::null(),
        )
    };
    check(code)
}

fn descriptor(sddl: &str) -> io::Result<Descriptor> {
    let text = wide(OsStr::new(sddl))?;
    let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `text` is NUL-terminated and lives past the call; `sd` is a valid out pointer and
    // the size out pointer may be null.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            ptr::null_mut(),
        )
    };
    if ok == 0 || sd.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(Descriptor(sd))
}

/// The access list inside `sd`. Only valid while `sd` lives.
fn dacl(sd: &Descriptor) -> io::Result<*const ACL> {
    let mut present = 0;
    let mut defaulted = 0;
    let mut acl: *mut ACL = ptr::null_mut();
    // SAFETY: `sd.0` is a valid descriptor owned by `sd`; the three out pointers are valid locals.
    let ok = unsafe { GetSecurityDescriptorDacl(sd.0, &mut present, &mut acl, &mut defaulted) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // A missing or null list means "everyone may do anything": never apply that.
    if present == 0 || acl.is_null() {
        return Err(io::Error::other(
            "the private access list could not be built",
        ));
    }
    Ok(acl)
}

fn check(code: WIN32_ERROR) -> io::Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code.cast_signed()))
    }
}

/// `s` as a NUL-terminated wide string. An interior NUL would silently name another path.
fn wide(s: &OsStr) -> io::Result<Vec<u16>> {
    let mut out: Vec<u16> = s.encode_wide().collect();
    if out.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the path contains a NUL character",
        ));
    }
    out.push(0);
    Ok(out)
}
