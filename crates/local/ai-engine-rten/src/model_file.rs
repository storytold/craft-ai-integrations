//! Reading a published `.onnx` file into memory with a ceiling, shared by every runner.
//!
//! `Model::load_file` trusts the file: a tiny file can name a huge tensor and have that length
//! reserved. A runner reads its files through [`read_bounded`] instead and loads the model from the
//! bytes alone, so no external-data file is ever opened and nothing larger than the runner's
//! ceiling is ever held. Each runner picks its own ceiling, well above its published export, and
//! turns [`Refused`] and the read errors into its own words.

use std::io::Read;
use std::path::Path;

/// Why a model file was not read.
#[derive(Debug)]
pub(crate) enum Refused {
    /// Not a plain file, or longer than the ceiling (before or while reading it).
    TooBig,
    /// The file could not be opened or read.
    Unreadable(std::io::Error),
}

/// A model file's bytes, from a plain file of at most `max` bytes.
///
/// The length is checked before reading, and the read itself stops one byte past `max`, so a file
/// that grows after the check, or reports a length it does not have, is still refused.
pub(crate) fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>, Refused> {
    let file = std::fs::File::open(path).map_err(Refused::Unreadable)?;
    let meta = file.metadata().map_err(Refused::Unreadable)?;
    if !meta.is_file() || meta.len() > max {
        return Err(Refused::TooBig);
    }
    read_capped(file, max)
}

/// At most `max` bytes from `source`, refused if it holds more. The read stops one byte past
/// `max`, so no more than that is ever held.
fn read_capped(source: impl Read, max: u64) -> Result<Vec<u8>, Refused> {
    let mut bytes = Vec::new();
    source
        .take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(Refused::Unreadable)?;
    if bytes.len() as u64 > max {
        return Err(Refused::TooBig);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file at the ceiling is read whole; one byte over is refused from its length.
    #[test]
    fn a_file_at_the_ceiling_is_read_and_one_byte_over_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.onnx");
        std::fs::write(&path, [7u8; 64]).unwrap();
        assert_eq!(read_bounded(&path, 64).unwrap(), vec![7u8; 64]);
        assert!(matches!(read_bounded(&path, 63), Err(Refused::TooBig)));
    }

    /// A sparse file far over the ceiling is refused without reading it (it uses no disk).
    #[test]
    fn a_huge_sparse_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.onnx");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(1 << 40)
            .unwrap();
        assert!(matches!(read_bounded(&path, 1 << 20), Err(Refused::TooBig)));
    }

    /// A folder in the file's place is refused as not a plain file. (Windows refuses to open one.)
    #[cfg(unix)]
    #[test]
    fn a_folder_in_the_files_place_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            read_bounded(dir.path(), 1 << 20),
            Err(Refused::TooBig)
        ));
    }

    /// What is read is checked again: more than the ceiling is refused whatever the length said.
    #[test]
    fn a_source_holding_more_than_the_ceiling_is_refused_once_read() {
        assert_eq!(read_capped(&[1u8; 8][..], 8).unwrap(), vec![1u8; 8]);
        assert!(matches!(
            read_capped(&[1u8; 9][..], 8),
            Err(Refused::TooBig)
        ));
        assert!(matches!(
            read_capped(std::io::repeat(0), 1 << 16),
            Err(Refused::TooBig)
        ));
    }

    /// A missing file is unreadable, not too big.
    #[test]
    fn a_missing_file_is_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let e = read_bounded(&dir.path().join("gone.onnx"), 64).unwrap_err();
        assert!(matches!(e, Refused::Unreadable(_)));
    }

    /// A file whose reported length (0) is less than what it holds is still stopped at the
    /// ceiling: the check after reading catches what the length check could not.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_file_that_holds_more_than_its_length_says_is_stopped_while_reading() {
        let path = Path::new("/proc/self/status");
        assert_eq!(std::fs::metadata(path).unwrap().len(), 0);
        assert!(matches!(read_bounded(path, 8), Err(Refused::TooBig)));
        assert!(read_bounded(path, 1 << 20).is_ok());
    }
}
