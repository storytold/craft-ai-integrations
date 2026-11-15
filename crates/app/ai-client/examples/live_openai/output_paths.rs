//! Checks the files a live example may write before a key or provider is used.

use std::path::Path;

/// Refuses output names that would overwrite the saved store or another output. This runs before
/// keychain access or a provider call: checking the store after writing cannot put its bytes back.
pub(super) fn validate(
    store: &Path,
    record: Option<&Path>,
    image: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(record) = record
        && same_file(record, store)?
    {
        return Err("--record must not name the shared connections store".into());
    }
    if let Some(image) = image
        && same_file(image, store)?
    {
        return Err("the image output must not name the shared connections store".into());
    }
    if let (Some(record), Some(image)) = (record, image)
        && same_file(record, image)?
    {
        return Err("--record and the image output name the same file".into());
    }
    Ok(())
}

/// Writes an image after the record may have been created.
pub(super) fn write_image(
    store: &Path,
    record: Option<&Path>,
    image: &Path,
    bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    // Creating the record can reveal a case-only or linked alias absent at preflight.
    validate(store, record, Some(image))?;
    std::fs::write(image, bytes)?;
    Ok(())
}

/// Whether two paths already identify one file, including symbolic links and hardlinks. Equal
/// names also collide before the output exists, once each parent folder is resolved. Both handles stay open during comparison
/// so an existing file's identity stays valid; a failure to read that identity is never "different".
fn same_file(a: &Path, b: &Path) -> std::io::Result<bool> {
    if resolved_name(a)? == resolved_name(b)? {
        return Ok(true);
    }
    let a = existing_handle(a)?;
    let b = existing_handle(b)?;
    Ok(matches!((a, b), (Some(a), Some(b)) if a == b))
}

/// `path` as an absolute name with its parent folder resolved, so a linked folder or `..` in the
/// parent gives the same name as the direct path. A parent that does not exist yet is left as
/// written: nothing can be linked through it.
fn resolved_name(path: &Path) -> std::io::Result<std::path::PathBuf> {
    let absolute = std::path::absolute(path)?;
    match (absolute.parent(), absolute.file_name()) {
        (Some(parent), Some(name)) => match parent.canonicalize() {
            Ok(parent) => Ok(parent.join(name)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(absolute),
            Err(error) => Err(error),
        },
        _ => Ok(absolute),
    }
}

/// A missing output may be created later. Every other failure to open an existing file or read
/// its identity refuses the run, because the preflight could not establish that writing is safe.
fn existing_handle(path: &Path) -> std::io::Result<Option<same_file::Handle>> {
    match same_file::Handle::from_path(path) {
        Ok(handle) => Ok(Some(handle)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[path = "output_paths/tests.rs"]
mod tests;
