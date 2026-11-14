//! The network itself: Depth Anything V2 Small on a DINOv2 ViT-S/14 backbone, both from
//! `candle-transformers`, on the processor with 32-bit weights (the files are half precision).
//!
//! Candle's layers are not `Send`, so the network lives on one thread of its own, built there
//! from the two weight files, and [`Estimate::estimate`] sends each picture to it and waits for
//! the map. That keeps the loaded model `Send` without any `unsafe`.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use candle_core::{DType, Device, Module, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::depth_anything_v2::{DepthAnythingV2, DepthAnythingV2Config};
use candle_transformers::models::dinov2;
use craft_ai_core::AiError;

use super::backend;
use super::prep::{DepthMap, MODEL_SIZE, postprocess, preprocess};

/// Something that turns a picture into a depth map: the network, or a stand-in in tests.
pub(super) trait Estimate: Send {
    /// Estimates relative depth for interleaved RGB (0..1, `w * h * 3`). The map may be any size.
    fn estimate(&self, rgb: &[f32], w: u32, h: u32) -> Result<DepthMap, AiError>;
}

fn be(e: candle_core::Error) -> AiError {
    backend(format!("The depth model failed: {e}"))
}

/// Depth Anything V2 Small, built from prepared variable builders.
struct Net {
    net: DepthAnythingV2,
    device: Device,
}

impl Net {
    fn new(vb_dino: VarBuilder, vb_head: VarBuilder) -> Result<Self, AiError> {
        let backbone = dinov2::vit_small(vb_dino).map_err(be)?;
        let net = DepthAnythingV2::new(
            Arc::new(backbone),
            DepthAnythingV2Config::vit_small(),
            vb_head,
        )
        .map_err(be)?;
        Ok(Self {
            net,
            device: Device::Cpu,
        })
    }

    /// One forward pass over a picture: relative depth at the preprocessed size (longer side 518,
    /// both sides multiples of 14), 0..1, larger = nearer.
    fn run(&self, rgb: &[f32], w: u32, h: u32) -> Result<DepthMap, AiError> {
        let prep = preprocess(rgb, w, h)?;
        let (pw, ph, side) = (
            prep.width as usize,
            prep.height as usize,
            MODEL_SIZE as usize,
        );
        let sq = pad_square(&prep.data, pw, ph, side);
        let input = Tensor::from_vec(sq, (1, 3, side, side), &self.device).map_err(be)?;
        let out = self.net.forward(&input).map_err(be)?;
        // The head's output grid need not equal the input (candle's DPT head gives 520 x 520 for a
        // 518 x 518 input): read its size from the tensor and map it back onto the square input.
        let dims = out.dims().to_vec();
        let (oh, ow) = match dims.as_slice() {
            [.., oh, ow] => (*oh, *ow),
            _ => return Err(backend(format!("unexpected output shape {dims:?}"))),
        };
        let flat = out
            .flatten_all()
            .map_err(be)?
            .to_vec1::<f32>()
            .map_err(be)?;
        if ow == 0 || oh == 0 || flat.len() != ow * oh {
            return Err(backend(format!("unexpected output shape {dims:?}")));
        }
        let cropped = crop_resample(&flat, (ow, oh), side, (pw, ph));
        postprocess(&cropped, prep.width, prep.height)
    }
}

/// The network input is square (518 x 518): pads planar `pw x ph` data by repeating the last row
/// and column.
fn pad_square(data: &[f32], pw: usize, ph: usize, side: usize) -> Vec<f32> {
    let mut sq = vec![0.0f32; 3 * side * side];
    for c in 0..3 {
        for y in 0..side {
            let sy = y.min(ph.saturating_sub(1));
            for x in 0..side {
                let sx = x.min(pw.saturating_sub(1));
                if let (Some(o), Some(v)) = (
                    sq.get_mut(c * side * side + y * side + x),
                    data.get(c * pw * ph + sy * pw + sx),
                ) {
                    *o = *v;
                }
            }
        }
    }
    sq
}

/// The `pw x ph` top-left part of a `side x side` input, sampled bilinearly (pixel centres) from
/// an `ow x oh` output that covers the whole square input.
pub(super) fn crop_resample(
    out: &[f32],
    (ow, oh): (usize, usize),
    side: usize,
    (pw, ph): (usize, usize),
) -> Vec<f32> {
    let (sx, sy) = (ow as f32 / side as f32, oh as f32 / side as f32);
    let at = |x: usize, y: usize| {
        out.get(y.min(oh.saturating_sub(1)) * ow + x.min(ow.saturating_sub(1)))
            .copied()
            .unwrap_or(0.0)
    };
    let mut res = Vec::with_capacity(pw * ph);
    for y in 0..ph {
        let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        for x in 0..pw {
            let fx = ((x as f32 + 0.5) * sx - 0.5).max(0.0);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let top = at(x0, y0) * (1.0 - tx) + at(x0 + 1, y0) * tx;
            let bottom = at(x0, y0 + 1) * (1.0 - tx) + at(x0 + 1, y0 + 1) * tx;
            res.push(top * (1.0 - ty) + bottom * ty);
        }
    }
    res
}

/// What the worker thread is asked: a picture, and where to send the answer.
struct Job {
    rgb: Vec<f32>,
    size: (u32, u32),
    reply: Sender<Result<DepthMap, AiError>>,
}

/// The network on its own thread.
pub(super) struct Worker {
    jobs: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    /// Builds the network from the two weight files' bytes (backbone, then head) on a new thread
    /// and returns once it is ready, or with the reason it could not be built. Nothing is
    /// memory-mapped. The bytes are dropped as soon as the network holds its own copy.
    pub fn start(dinov2: Vec<u8>, head: Vec<u8>) -> Result<Self, AiError> {
        let (jobs, inbox) = channel::<Job>();
        let (ready, built) = channel::<Result<(), AiError>>();
        let thread = std::thread::Builder::new()
            .name("craft-depth".into())
            .stack_size(16 * 1024 * 1024)
            .spawn(move || serve(dinov2, head, &ready, &inbox))
            .map_err(|_| backend("The depth model could not start a thread."))?;
        let worker = Self {
            jobs: Some(jobs),
            thread: Some(thread),
        };
        built
            .recv()
            .map_err(|_| backend("The depth model stopped while loading."))??;
        Ok(worker)
    }
}

/// The worker thread's body: build the network, say so, then answer pictures until the sender is
/// dropped.
fn serve(
    dinov2: Vec<u8>,
    head: Vec<u8>,
    ready: &Sender<Result<(), AiError>>,
    inbox: &Receiver<Job>,
) {
    let built = (|| {
        let device = Device::Cpu;
        let vb_dino =
            VarBuilder::from_buffered_safetensors(dinov2, DType::F32, &device).map_err(be)?;
        let vb_head =
            VarBuilder::from_buffered_safetensors(head, DType::F32, &device).map_err(be)?;
        Net::new(vb_dino, vb_head)
    })();
    let net = match built {
        Ok(net) => {
            let _ = ready.send(Ok(()));
            net
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    for job in inbox {
        let (w, h) = job.size;
        let _ = job.reply.send(net.run(&job.rgb, w, h));
    }
}

impl Estimate for Worker {
    fn estimate(&self, rgb: &[f32], w: u32, h: u32) -> Result<DepthMap, AiError> {
        let stopped = || backend("The depth model stopped unexpectedly.");
        let (reply, answer) = channel();
        self.jobs
            .as_ref()
            .ok_or_else(stopped)?
            .send(Job {
                rgb: rgb.to_vec(),
                size: (w, h),
                reply,
            })
            .map_err(|_| stopped())?;
        answer.recv().map_err(|_| stopped())?
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Closing the channel ends the thread's loop; wait for it so the weights are freed when
        // the model is unloaded.
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests;
