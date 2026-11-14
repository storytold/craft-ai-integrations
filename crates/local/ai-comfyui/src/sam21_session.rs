//! SAM 2.1 subject selection: Florence-2 detects boxes, SAM 2.1 segments them.
use crate::http::Http;
use craft_ai_core::endpoint::Endpoint;
use craft_ai_core::{AiError, ErrorKind};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const REQUIRED: &[&str] = &[
    "LoadImage",
    "Florence2ModelLoader",
    "Florence2Run",
    "Florence2toCoordinates",
    "DownloadAndLoadSAM2Model",
    "Sam2Segmentation",
    "MaskToImage",
    "PreviewImage",
];
struct Prepared {
    filename: String,
    width: u32,
    height: u32,
    device: &'static str,
    precision: &'static str,
}

/// The device ComfyUI reports, and the precision that is safe on it. Half precision is only used
/// on CUDA; CPU and Apple graphics get full precision because half is not supported on CPU and is
/// untested on MPS. Anything else is refused before an image is uploaded.
fn device_for(stats: &Value) -> Result<(&'static str, &'static str), AiError> {
    match stats.pointer("/devices/0/type").and_then(Value::as_str) {
        Some("cuda") => Ok(("cuda", "fp16")),
        Some("mps") => Ok(("mps", "fp32")),
        Some("cpu") => Ok(("cpu", "fp32")),
        _ => Err(AiError::unsupported(
            "SAM 2.1 needs ComfyUI to report a CUDA, Apple (MPS) or CPU device",
        )),
    }
}

/// Cancels only the in-flight prompt owned by this SAM 2.1 session.
#[derive(Clone, Debug, Default)]
pub struct Sam21Canceller(Arc<AtomicU64>);

impl Sam21Canceller {
    /// Cancels the prompt this session is waiting on.
    pub fn cancel(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    /// The current cancel count, noted before a prompt is queued.
    fn mark(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    /// Whether a cancel arrived after `mark` was taken.
    fn fired_since(&self, mark: u64) -> bool {
        self.0.load(Ordering::SeqCst) != mark
    }
}

/// Blocking local session. Never call it on the drawing thread.
pub struct Sam21Session {
    http: Http,
    prepared: Option<Prepared>,
    canceller: Sam21Canceller,
}
impl Sam21Session {
    /// Opens a session on a loopback ComfyUI address. Nothing is sent yet.
    pub fn new(base: &str) -> Result<Self, AiError> {
        let endpoint =
            Endpoint::parse(base).map_err(|_| AiError::invalid("invalid ComfyUI address"))?;
        if !endpoint.is_loopback() || base != endpoint.fingerprint() {
            return Err(AiError::invalid(
                "SAM 2.1 needs a loopback ComfyUI address without a path",
            ));
        }
        Ok(Self {
            http: Http::new(
                base.to_owned(),
                Duration::from_secs(3),
                Duration::from_secs(15),
            ),
            prepared: None,
            canceller: Sam21Canceller::default(),
        })
    }
    /// A handle that cancels this session's current prompt from another thread.
    pub fn canceller(&self) -> Sam21Canceller {
        self.canceller.clone()
    }
    /// Confirms the port is serving ComfyUI and nothing else.
    pub fn verify_server(&self) -> Result<(), AiError> {
        let stats = self.http.get_json("/system_stats")?;
        if stats
            .pointer("/system/comfyui_version")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(AiError::unsupported(
                "The local port is not serving ComfyUI",
            ));
        }
        Ok(())
    }
    /// Checks the device and installed nodes, then uploads the image for later selections.
    pub fn prepare(&mut self, image: &[u8]) -> Result<(), AiError> {
        self.prepared = None;
        if image.len() < 8
            || image.len() > 16 * 1024 * 1024
            || !image.starts_with(b"\x89PNG\r\n\x1a\n")
        {
            return Err(AiError::invalid("SAM 2.1 needs a PNG image under 16 MiB"));
        }
        let reader = png::Decoder::new(std::io::Cursor::new(image))
            .read_info()
            .map_err(|_| AiError::invalid("SAM 2.1 needs a valid PNG image"))?;
        let (width, height) = (reader.info().width, reader.info().height);
        if width == 0
            || height == 0
            || width > 8192
            || height > 8192
            || u64::from(width) * u64::from(height) > 16_777_216
        {
            return Err(AiError::invalid(
                "SAM 2.1 source image dimensions are invalid",
            ));
        }
        let (device, precision) = device_for(&self.http.get_json("/system_stats")?)?;
        let nodes = self.http.get_object_info()?;
        for class in REQUIRED {
            if nodes.get(*class).is_none() {
                return Err(AiError::unsupported(format!(
                    "ComfyUI is missing the {class} node"
                )));
            }
        }
        crate::detector::require_detector(&nodes)?;
        let filename = self.http.upload_image("sam21-select", "image/png", image)?;
        self.prepared = Some(Prepared {
            filename,
            width,
            height,
            device,
            precision,
        });
        Ok(())
    }
    /// Selects the subject named by `text` and returns a PNG mask the size of the source.
    pub fn ground(&self, text: &str) -> Result<Vec<u8>, AiError> {
        let p = self
            .prepared
            .as_ref()
            .ok_or_else(|| AiError::invalid("SAM 2.1 image not prepared"))?;
        let text = text.trim();
        if text.is_empty() || text.len() > 256 || text.chars().any(char::is_control) {
            return Err(AiError::invalid("SAM 2.1 needs a short subject name"));
        }
        static NEXT_SELECTION: AtomicU64 = AtomicU64::new(0);
        // A new sink name for every selection: an identical graph without a client id is served from
        // the server's cache with no outputs in its history (ComfyUI 0.22.3), and the selection would fail.
        let sink = format!(
            "craft-sam21-{}-{}",
            std::process::id(),
            NEXT_SELECTION.fetch_add(1, Ordering::Relaxed)
        );
        let graph = json!({
            "1":{"class_type":"LoadImage","inputs":{"image":p.filename}},
            "2":{"class_type":"Florence2ModelLoader","inputs":{"model":crate::detector::FLORENCE2_DETECTOR,"precision":p.precision,"convert_to_safetensors":false}},
            "3":{"class_type":"Florence2Run","inputs":{"image":["1",0],"florence2_model":["2",0],"text_input":text,"task":"caption_to_phrase_grounding","fill_mask":true,"keep_model_loaded":true}},
            "4":{"class_type":"Florence2toCoordinates","inputs":{"data":["3",3],"index":"","batch":false}},
            "5":{"class_type":"DownloadAndLoadSAM2Model","inputs":{"model":"sam2.1_hiera_large.safetensors","segmentor":"single_image","device":p.device,"precision":p.precision}},
            "6":{"class_type":"Sam2Segmentation","inputs":{"sam2_model":["5",0],"image":["1",0],"keep_model_loaded":true,"bboxes":["4",1],"individual_objects":true}},
            "7":{"class_type":"MaskToImage","inputs":{"mask":["6",0]}},
            "8":{"class_type":"SaveImage","inputs":{"images":["7",0],"filename_prefix":sink}}
        });
        let mut graph = graph;
        crate::scratch::route_sinks(&mut graph)?;
        let started = self.canceller.mark();
        let response = self.http.post("/prompt", &json!({"prompt":graph}))?;
        let id = response
            .get("prompt_id")
            .and_then(Value::as_str)
            .filter(|s| {
                !s.is_empty()
                    && s.len() <= 128
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            })
            .ok_or_else(|| {
                AiError::new(
                    ErrorKind::Provider,
                    "ComfyUI did not return a valid prompt id",
                )
            })?;
        let deadline = Instant::now() + Duration::from_secs(240);
        loop {
            if self.cancelled(id, started) {
                return Err(AiError::new(
                    ErrorKind::Cancelled,
                    "SAM 2.1 request cancelled",
                ));
            }
            let history = self.http.get_json(&format!("/history/{id}"))?;
            if self.cancelled(id, started) {
                return Err(AiError::new(
                    ErrorKind::Cancelled,
                    "SAM 2.1 request cancelled",
                ));
            }
            if let Some(record) = history.get(id) {
                if record.pointer("/status/status_str").and_then(Value::as_str) == Some("error") {
                    return Err(AiError::new(
                        ErrorKind::Provider,
                        "SAM 2.1 workflow failed in ComfyUI",
                    ));
                }
                if record.pointer("/status/completed").and_then(Value::as_bool) == Some(true) {
                    let output = record.pointer("/outputs/8").ok_or_else(|| {
                        AiError::new(ErrorKind::Provider, "SAM 2.1 returned no selection mask")
                    })?;
                    let masks = crate::sam3_masks::fetch(&self.http, output, p.width, p.height)
                        .map_err(|e| AiError::new(e.kind, e.message.replace("SAM 3", "SAM 2.1")))?;
                    if self.cancelled(id, started) {
                        return Err(AiError::new(
                            ErrorKind::Cancelled,
                            "SAM 2.1 request cancelled",
                        ));
                    }
                    if !masks.has_pixels()? {
                        return Err(AiError::new(
                            ErrorKind::Provider,
                            format!("Nothing matched {text}"),
                        ));
                    }
                    return masks.union_png();
                }
            }
            if Instant::now() >= deadline {
                return Err(AiError::new(
                    ErrorKind::Network,
                    "SAM 2.1 timed out in ComfyUI",
                ));
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// True once a cancel arrived since `started`. It then also removes this session's own queued prompt.
    fn cancelled(&self, id: &str, started: u64) -> bool {
        if !self.canceller.fired_since(started) {
            return false;
        }
        let _ = self.http.post("/queue", &json!({"delete":[id]}));
        true
    }
}
