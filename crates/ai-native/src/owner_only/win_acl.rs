//! The workspace's one `unsafe` exception, named in `AGENTS.md` and `CONTRIBUTING.md`: the Win32
//! calls that create a file or folder with an owner-only access list. Std cannot pass a security
//! descriptor when it creates a file.
//!
//! Only safe functions leave this module. Each `unsafe` block is one foreign call whose pointers
//! come from values that outlive it, and `tests/unsafe_exception.rs` fails if `unsafe` is allowed
//! anywhere else.
//!
//! The list is set as the object is created, never afterwards: a list applied later cannot revoke
//! a handle another account opened in between. It is protected (`P`, nothing inherited from the
//! parent) and grants full control (`FA`) to two names only: the user this process runs as, by
//! SID, and `SYSTEM`. The user SID is named rather than `OWNER RIGHTS`, because an elevated
//! process may make `Administrators` the owner, and the same person running unelevated later
//! must still open the file. A folder's entries pass to what is created inside it (`OICI`).
//!
//! Limit: a volume that keeps no access lists (FAT, exFAT, some network shares) stores the file
//! without one, as a Unix mount without modes ignores `0600`. The save still goes ahead, as it
//! does on Unix: refusing would leave a portable install on such a drive unable to save at all,
//! and the secret file there is encrypted in any case.
#![allow(unsafe_code)]

use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_GENERIC_WRITE,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Creates a new file that only this user and SYSTEM can open, opened for writing with no
/// sharing. Fails with `AlreadyExists` if `path` exists: an existing file is never taken over.
pub(super) fn create_file(path: &Path) -> io::Result<File> {
    let name = wide_path(path)?;
    let sd = descriptor(&file_sddl(&user_sid()?))?;
    let attributes = sd.attributes();
    // SAFETY: `name` is NUL-terminated and outlives the call; `attributes` points at `sd`'s
    // descriptor, alive until the end of this function; the template handle may be null.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            // A link planted at the name counts as the name, so CREATE_NEW fails on it instead
            // of creating the file the link points at.
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `handle` is a valid file handle that nothing else owns; `File` closes it once.
    Ok(unsafe { File::from_raw_handle(handle as RawHandle) })
}

/// Creates one folder (its parent must exist) that only this user and SYSTEM can open, and
/// whose entries pass to everything created inside it. Fails with `AlreadyExists` if it exists.
pub(super) fn create_dir(path: &Path) -> io::Result<()> {
    let name = wide_path(path)?;
    let sd = descriptor(&dir_sddl(&user_sid()?))?;
    let attributes = sd.attributes();
    // SAFETY: `name` is NUL-terminated and outlives the call; `attributes` points at `sd`'s
    // descriptor, alive until the end of this function.
    let ok = unsafe { CreateDirectoryW(name.as_ptr(), &attributes) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Owner-only list for one file, owned by `sid`.
pub(super) fn file_sddl(sid: &str) -> String {
    format!("O:{sid}D:P(A;;FA;;;{sid})(A;;FA;;;SY)")
}

/// Owner-only list for a folder, passed on to everything created inside it.
pub(super) fn dir_sddl(sid: &str) -> String {
    format!("O:{sid}D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)")
}

/// A security descriptor the system allocated; freed exactly once, on drop.
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(u32::MAX),
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from ConvertStringSecurityDescriptorToSecurityDescriptorW or
        // GetNamedSecurityInfoW, both of which allocate with LocalAlloc, and this is the only
        // place it is freed.
        unsafe { LocalFree(self.0) };
    }
}

fn descriptor(sddl: &str) -> io::Result<Descriptor> {
    let text = wide(OsStr::new(sddl))?;
    let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `text` is NUL-terminated and outlives the call; `sd` is a valid out pointer and
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

/// A token handle, closed exactly once on drop.
struct Token(HANDLE);

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from OpenProcessToken and is closed only here.
        unsafe { CloseHandle(self.0) };
    }
}

/// The SID of the user this process runs as, such as `S-1-5-21-…-1001`.
pub(super) fn user_sid() -> io::Result<String> {
    let mut raw: HANDLE = ptr::null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo handle that needs no closing; `raw` is a valid
    // out pointer.
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Token(raw);
    // TOKEN_USER holds a pointer into the same buffer, so the buffer is u64-aligned and sized
    // well past the largest SID (68 bytes) plus the header.
    let mut buffer = [0u64; 64];
    let mut needed = 0u32;
    let size = u32::try_from(size_of_val(&buffer)).unwrap_or(u32::MAX);
    // SAFETY: `buffer` is writable for `size` bytes and outlives every use of its contents;
    // `needed` is a valid out pointer.
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: on success the buffer starts with a TOKEN_USER, and it is suitably aligned.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text: windows_sys::core::PWSTR = ptr::null_mut();
    // SAFETY: `sid` points into `buffer`, still alive; `text` is a valid out pointer.
    let ok = unsafe { ConvertSidToStringSidW(sid, &mut text) };
    if ok == 0 || text.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(take_local_string(text))
}

/// Copies a NUL-terminated wide string the system allocated with LocalAlloc, then frees it.
fn take_local_string(text: windows_sys::core::PWSTR) -> String {
    let mut len = 0;
    // SAFETY: the caller passes a non-null, NUL-terminated string; reading up to its NUL stays
    // in bounds.
    while unsafe { *text.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `text` is valid for `len` elements, as just counted.
    let out = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
    // SAFETY: the string came from LocalAlloc and is freed once, here.
    unsafe { LocalFree(text.cast()) };
    out
}

/// The owner and access list of `path` as SDDL, read in-process. [`super::check_private_dir`]
/// judges an existing folder by it, and the tests check what the creation calls stored with it
/// (PowerShell's `Get-Acl` failed on the `windows-latest` CI runner).
pub(super) fn sddl_of(path: &Path) -> io::Result<String> {
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION};
    let name = wide_path(path)?;
    let info = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `name` is NUL-terminated and outlives the call; the owner, group and list out
    // pointers may be null when `sd` is requested; `sd` is a valid out pointer.
    let code = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            info,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut sd,
        )
    };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code.cast_signed()));
    }
    let sd = Descriptor(sd);
    let mut text: windows_sys::core::PWSTR = ptr::null_mut();
    // SAFETY: `sd.0` is a valid descriptor owned by `sd`; `text` is a valid out pointer and the
    // length out pointer may be null.
    let ok = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            sd.0,
            SDDL_REVISION_1,
            info,
            &mut text,
            ptr::null_mut(),
        )
    };
    if ok == 0 || text.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(take_local_string(text))
}

/// `path` as a NUL-terminated wide string the creation calls accept past the old 260-character
/// limit: made absolute, then, when long, given the `\\?\` prefix (`\\?\UNC\` for a share). Std
/// does the same for its own file calls, which these two replace.
fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
    // MAX_PATH less room for an 8.3 name, the limit for folders.
    const LIMIT: usize = 247;
    let full = std::path::absolute(path)?;
    let name: Vec<u16> = full.as_os_str().encode_wide().collect();
    let verbatim: Vec<u16> = r"\\?\".encode_utf16().collect();
    let share: Vec<u16> = r"\\".encode_utf16().collect();
    let prefixed = if name.len() <= LIMIT || name.starts_with(&verbatim) {
        name
    } else if let Some(rest) = name.strip_prefix(share.as_slice()) {
        // \\server\share\x becomes \\?\UNC\server\share\x
        r"\\?\UNC\"
            .encode_utf16()
            .chain(rest.iter().copied())
            .collect()
    } else {
        verbatim.into_iter().chain(name).collect()
    };
    terminate(prefixed)
}

/// `s` as a NUL-terminated wide string. An interior NUL would silently name another path.
fn wide(s: &OsStr) -> io::Result<Vec<u16>> {
    terminate(s.encode_wide().collect())
}

fn terminate(mut out: Vec<u16>) -> io::Result<Vec<u16>> {
    if out.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the path contains a NUL character",
        ));
    }
    out.push(0);
    Ok(out)
}
