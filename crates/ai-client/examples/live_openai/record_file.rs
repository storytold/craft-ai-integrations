//! Appends run records to a JSON-lines file, one record per line.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Appends `line` and a newline, so several runs collect in one file. When the file already ends
/// without a newline (written by hand or cut short), one is added first, so the new record never
/// joins the last one on a single unreadable line.
pub(super) fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(path)?;
    let len = file.metadata()?.len();
    if len > 0 {
        let mut last = [0u8; 1];
        file.seek(SeekFrom::Start(len - 1))?;
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            file.write_all(b"\n")?;
        }
    }
    writeln!(file, "{line}")
}

#[cfg(test)]
#[path = "record_file/tests.rs"]
mod tests;
