//! Graphics memory and free disk, best effort. `None` means unknown, never zero.

use std::path::Path;

/// Total memory of the largest Nvidia GPU, from `nvidia-smi`. `None` without one.
pub fn vram_mb() -> Option<u32> {
    let mut command = std::process::Command::new("nvidia-smi");
    command.args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // no console window
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_nvidia_smi(&String::from_utf8_lossy(&output.stdout))
}

/// The largest figure in `nvidia-smi` CSV output (one MiB value per line).
pub fn parse_nvidia_smi(text: &str) -> Option<u32> {
    text.lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .filter(|mb| *mb > 0)
        .max()
}

/// Bytes free on the disk holding `path`.
pub fn free_disk_bytes(path: &Path) -> Option<u64> {
    fs4::available_space(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_smi_output_is_parsed() {
        assert_eq!(parse_nvidia_smi("24576\n"), Some(24_576));
        assert_eq!(parse_nvidia_smi("8192\r\n24576\r\n"), Some(24_576));
        assert_eq!(parse_nvidia_smi(""), None);
        assert_eq!(parse_nvidia_smi("N/A\n"), None);
        assert_eq!(parse_nvidia_smi("0\n"), None);
    }
}
