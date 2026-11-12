//! Small file helpers shared by the secret file and the shared store: bounded reads, atomic
//! writes, and a cross-process lock. Private to the crate.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::owner_only;

/// How long to wait for another process to finish writing before giving up.
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// Reads a whole file, refusing anything larger than `max` bytes. `Ok(None)` if it is absent.
pub(crate) fn read_capped(path: &Path, max: usize) -> io::Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut buf = Vec::new();
    // Read one byte past the cap so an oversize file is detected without loading all of it.
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    file.take(limit).read_to_end(&mut buf)?;
    if buf.len() > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("file is larger than the {max} byte limit"),
        ));
    }
    Ok(Some(buf))
}

fn file_name_or_default(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "store".to_owned())
}

fn ensure_parent(path: &Path) -> io::Result<()> {
    match path.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(dir) => owner_only::create_dir_all(dir),
        None => Ok(()),
    }
}

/// Writes `bytes` to `path` so a reader sees the old file or the new one, never a mix. The temp
/// file is new, private to the user (mode 0600 on Unix, an owner-only access list on Windows)
/// and flushed before the rename.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    ensure_parent(path)?;
    let name = file_name_or_default(path);
    let tmp = path.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
    let result = write_tmp(&tmp, bytes).and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn write_tmp(tmp: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = match owner_only::create_file(tmp) {
        // A crash can leave a temp file under this process id. It is removed, never reused: a file
        // someone else made first would keep their ownership and so their access.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            fs::remove_file(tmp)?;
            owner_only::create_file(tmp)?
        }
        other => other?,
    };
    file.write_all(bytes)?;
    file.sync_all()
}

fn lock_path(path: &Path) -> PathBuf {
    path.with_file_name(format!("{}.lock", file_name_or_default(path)))
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path(path))
}

/// Runs `f` while holding an exclusive lock on a sibling `.lock` file of `path`.
pub(crate) fn with_lock<T>(path: &Path, f: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    ensure_parent(path)?;
    let lock = open_lock_file(path)?;
    let start = Instant::now();
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(fs::TryLockError::WouldBlock) => {
                if start.elapsed() > LOCK_TIMEOUT {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "another process is writing the file",
                    ));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(fs::TryLockError::Error(e)) => return Err(e),
        }
    }
    let out = f();
    let _ = lock.unlock();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_capped_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_capped(&dir.path().join("x"), 10).unwrap().is_none());
    }

    #[test]
    fn read_capped_refuses_oversize() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big");
        fs::write(&p, vec![b'a'; 11]).unwrap();
        assert!(read_capped(&p, 10).is_err());
        fs::write(&p, vec![b'a'; 10]).unwrap();
        assert_eq!(read_capped(&p, 10).unwrap().unwrap().len(), 10);
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nested").join("f.json");
        atomic_write(&p, b"one").unwrap();
        atomic_write(&p, b"two").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        let leftovers: Vec<_> = fs::read_dir(p.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn lock_is_exclusive_while_held_and_released_after() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.json");
        let blocked = with_lock(&p, || {
            let other = open_lock_file(&p)?;
            Ok(matches!(
                other.try_lock(),
                Err(fs::TryLockError::WouldBlock)
            ))
        })
        .unwrap();
        assert!(blocked);
        assert!(with_lock(&p, || Ok(true)).unwrap());
    }

    /// A readable temp file left at this process's temp name is replaced, not written through, so
    /// its loose mode never reaches the saved file.
    #[cfg(unix)]
    #[test]
    fn a_leftover_temp_file_does_not_loosen_the_saved_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("secrets.enc.json");
        let tmp = dir
            .path()
            .join(format!(".secrets.enc.json.tmp-{}", std::process::id()));
        fs::write(&tmp, b"stale").unwrap();
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644)).unwrap();
        atomic_write(&p, b"new").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"new");
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
