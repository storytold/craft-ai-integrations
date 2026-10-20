//! Turns an install into an [`Inventory`]: model files by folder, node pack folders, graphics
//! memory and free disk. Reads metadata only; nothing is opened or hashed.

use crate::detect::ComfyInstall;
use crate::hardware;
use craft_ai_features::{InstalledFile, Inventory};
use std::path::Path;

const MODEL_EXTENSIONS: &[&str] = &[
    "safetensors",
    "sft",
    "pt",
    "pth",
    "ckpt",
    "gguf",
    "onnx",
    "bin",
];
/// Small text files a bundled model needs beside its weights (config, tokenizer, loader code).
/// Only seen up to [`SMALL_FILE_LIMIT`] bytes, so a folder of notes or datasets is not listed.
const SMALL_EXTENSIONS: &[&str] = &["json", "py", "txt"];
const SMALL_FILE_LIMIT: u64 = 2 * 1024 * 1024;
const MAX_DEPTH: usize = 5;
const MAX_FILES: usize = 20_000;

/// What is installed. Walks the model folders and `custom_nodes` once.
pub fn scan(install: &ComfyInstall) -> Inventory {
    let mut files = Vec::new();
    for (folder, dir) in &install.model_dirs {
        walk(dir, dir, 0, folder, &mut files);
    }
    let mut node_folders: Vec<String> = std::fs::read_dir(&install.custom_nodes)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') && n != "__pycache__")
        .map(|n| n.to_ascii_lowercase())
        .collect();
    node_folders.sort();
    Inventory {
        comfyui_found: true,
        files,
        node_folders,
        vram_mb: hardware::vram_mb(),
        free_disk_bytes: hardware::free_disk_bytes(&install.base),
    }
}

fn walk(root: &Path, dir: &Path, depth: usize, folder: &str, out: &mut Vec<InstalledFile>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        if out.len() >= MAX_FILES {
            return;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            walk(root, &path, depth + 1, folder, out);
        } else if kind.is_file()
            && let Some(ext) = path.extension().and_then(|e| e.to_str())
            && let Ok(meta) = entry.metadata()
            && (MODEL_EXTENSIONS.iter().any(|m| m.eq_ignore_ascii_case(ext))
                || (meta.len() <= SMALL_FILE_LIMIT
                    && SMALL_EXTENSIONS.iter().any(|m| m.eq_ignore_ascii_case(ext))))
            && let Ok(relative) = path.strip_prefix(root)
        {
            out.push(InstalledFile {
                folder: folder.to_owned(),
                filename: relative
                    .components()
                    .filter_map(|c| c.as_os_str().to_str())
                    .collect::<Vec<_>>()
                    .join("/"),
                size_bytes: meta.len(),
            });
        }
    }
}
