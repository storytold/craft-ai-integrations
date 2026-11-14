//! Which model file formats are installed, judged by the name and by the first bytes.
//!
//! - **Safetensors, GGUF and ONNX** hold numbers, not code, and are accepted.
//! - **Pickle formats** (`.ckpt`, `.pt`, `.pth`, `.bin`, `.pkl`, `.pickle`) can run arbitrary code
//!   when a program loads them. They are installed only with the person's explicit consent.
//! - **Anything else** is refused.
//! - A file must be what its name says. Pickle bytes saved as `model.safetensors` are the
//!   obvious disguise, so the first bytes are read and a mismatch is refused.

use serde::{Deserialize, Serialize};

/// The longest safetensors header that is believed. The real limit is generous; a number beyond
/// this is not a header.
const MAX_SAFETENSORS_HEADER: u64 = 100 * 1024 * 1024;

/// The first bytes needed to recognise any format here.
pub const SNIFF_BYTES: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Safetensors,
    Gguf,
    Onnx,
    Pickle,
    Unknown,
}

/// Why a file cannot be installed as it is named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatProblem {
    /// The name is not one of the accepted kinds.
    UnsupportedType,
    /// The bytes are not what the name says.
    Mislabelled { claims: Format, found: Format },
}

impl Format {
    /// The format the file name claims, from its last extension.
    pub fn from_name(name: &str) -> Self {
        let Some((_, extension)) = name.rsplit_once('.') else {
            return Self::Unknown;
        };
        match extension.to_ascii_lowercase().as_str() {
            "safetensors" => Self::Safetensors,
            "gguf" => Self::Gguf,
            "onnx" => Self::Onnx,
            "ckpt" | "pt" | "pth" | "bin" | "pkl" | "pickle" => Self::Pickle,
            _ => Self::Unknown,
        }
    }

    /// What the first bytes look like. ONNX has no reliable signature, so it is never returned.
    pub fn sniff(head: &[u8]) -> Self {
        if head.starts_with(b"GGUF") {
            return Self::Gguf;
        }
        // A PyTorch checkpoint is a zip; older ones are a bare pickle stream (protocol marker).
        if head.starts_with(b"PK\x03\x04") || matches!(head, [0x80, 2..=5, ..]) {
            return Self::Pickle;
        }
        if let Some(length) = head.get(..8).and_then(|b| <[u8; 8]>::try_from(b).ok())
            && u64::from_le_bytes(length) <= MAX_SAFETENSORS_HEADER
            && head.get(8) == Some(&b'{')
        {
            return Self::Safetensors;
        }
        Self::Unknown
    }

    /// Checks a file named `name` whose first bytes are `head`.
    pub fn check(name: &str, head: &[u8]) -> Result<Self, FormatProblem> {
        let claims = Self::from_name(name);
        let found = Self::sniff(head);
        match claims {
            Self::Unknown => Err(FormatProblem::UnsupportedType),
            // A pickle format may hold anything; it is the consent that matters.
            Self::Pickle => Ok(claims),
            Self::Safetensors | Self::Gguf if found != claims => {
                Err(FormatProblem::Mislabelled { claims, found })
            }
            // No signature to read, so only an obvious other format is a mismatch.
            Self::Onnx if found != Self::Unknown => {
                Err(FormatProblem::Mislabelled { claims, found })
            }
            _ => Ok(claims),
        }
    }

    /// Loading a file of this format can run code from the file.
    pub fn runs_code_when_loaded(self) -> bool {
        self == Self::Pickle
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Safetensors => "safetensors",
            Self::Gguf => "GGUF",
            Self::Onnx => "ONNX",
            Self::Pickle => "pickle",
            Self::Unknown => "unknown",
        }
    }
}
