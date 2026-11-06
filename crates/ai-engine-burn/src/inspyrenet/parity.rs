//! Parity with the PyTorch reference implementation of InSPyReNet.
//!
//! Opt-in: it needs the published checkpoint and a reference file (input, every intermediate
//! stage and the prediction, computed once by PyTorch on a synthetic 384² input with
//! `tools/burn/inspyrenet_reference.py`):
//!
//! ```text
//! CRAFT_TEST_INSPYRENET_WEIGHTS=…/inspyrenet.safetensors \
//! CRAFT_TEST_INSPYRENET_REFERENCE=…/inspyrenet-ref-384.safetensors \
//! cargo test --release -p craft-ai-engine-burn -j 8 --lib parity -- --ignored --nocapture
//! ```
//!
//! Each stage is compared on its own, so a mismatch points at the module that causes it.

use std::collections::HashMap;
use std::path::PathBuf;

use burn::backend::wgpu::WgpuDevice;
use burn::prelude::*;
use burn::tensor::TensorData;

use super::net::InSPyReNet;
use super::{on_big_stack, weights};

struct Ref {
    shape: Vec<usize>,
    data: Vec<f32>,
}

/// A float32 safetensors file, read without a dependency.
fn read_reference(path: &str) -> HashMap<String, Ref> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
    let n = u64::from_le_bytes(bytes[..8].try_into().unwrap()) as usize;
    let header: serde_json::Value = serde_json::from_slice(&bytes[8..8 + n]).unwrap();
    let data = &bytes[8 + n..];
    let mut out = HashMap::new();
    for (k, v) in header.as_object().unwrap() {
        if k == "__metadata__" {
            continue;
        }
        assert_eq!(v["dtype"], "F32", "{k}");
        let shape = v["shape"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d.as_u64().unwrap() as usize)
            .collect();
        let a = v["data_offsets"][0].as_u64().unwrap() as usize;
        let b = v["data_offsets"][1].as_u64().unwrap() as usize;
        let floats = data[a..b]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        out.insert(
            k.clone(),
            Ref {
                shape,
                data: floats,
            },
        );
    }
    out
}

/// `(max |Δ|, max |reference|)`, printed with the mean difference.
fn compare(name: &str, got: Tensor<4>, want: &Ref) -> (f32, f32) {
    assert_eq!(got.dims().to_vec(), want.shape, "{name}: shape");
    let got = got.into_data().try_to_vec::<f32>().unwrap();
    let (mut max, mut sum, mut scale) = (0.0f32, 0.0f64, 0.0f32);
    for (a, b) in got.iter().zip(&want.data) {
        let d = (a - b).abs();
        max = max.max(d);
        sum += d as f64;
        scale = scale.max(b.abs());
    }
    let mean = sum / got.len().max(1) as f64;
    println!("{name:<12} max |Δ| {max:<12.3e} mean |Δ| {mean:<12.3e} max |ref| {scale:.3}");
    (max, scale)
}

fn run(device: Device) {
    let (Some(weights_path), Ok(reference)) = (
        std::env::var_os("CRAFT_TEST_INSPYRENET_WEIGHTS").map(PathBuf::from),
        std::env::var("CRAFT_TEST_INSPYRENET_REFERENCE"),
    ) else {
        panic!("set CRAFT_TEST_INSPYRENET_WEIGHTS and CRAFT_TEST_INSPYRENET_REFERENCE");
    };
    let r = read_reference(&reference);
    on_big_stack(move || {
        let net = weights::fill(InSPyReNet::new(&device), &weights_path)?;
        let input = &r["input"];
        let x = Tensor::<4>::from_data(
            TensorData::new(input.data.clone(), input.shape.clone()),
            &device,
        );
        let parts = net.forward_parts(x.clone(), &|| false).unwrap();
        let pred = net.forward(x, &|| false).unwrap();

        let mut worst_rel = 0.0f32;
        let stages = parts
            .backbone
            .into_iter()
            .enumerate()
            .map(|(i, t)| (format!("backbone.{i}"), t))
            .chain(
                parts
                    .laplacian
                    .into_iter()
                    .enumerate()
                    .map(|(i, t)| (format!("laplacian.{i}"), t)),
            )
            .chain(
                parts
                    .saliency
                    .into_iter()
                    .enumerate()
                    .map(|(i, t)| (format!("saliency.{i}"), t)),
            );
        for (name, t) in stages {
            let (max, scale) = compare(&name, t, &r[&name]);
            worst_rel = worst_rel.max(max / scale.max(1e-6));
        }
        let want = &r["pred"];
        let got = pred.into_data().try_to_vec::<f32>().unwrap();
        let (mut max, mut flips) = (0.0f32, 0usize);
        for (a, b) in got.iter().zip(&want.data) {
            max = max.max((a - b).abs());
            flips += usize::from((*a >= 0.5) != (*b >= 0.5));
        }
        println!(
            "pred         max |Δ| {max:.3e}, {flips} of {} pixels change side at 0.5",
            got.len()
        );
        println!("worst relative difference over the stages: {worst_rel:.3e}");
        assert!(
            worst_rel < 5e-3,
            "a stage drifts from PyTorch by {worst_rel:.3e} of its range"
        );
        assert!(max < 1e-2, "the prediction differs by up to {max}");
        assert!(flips * 10_000 <= got.len(), "{flips} pixels change side");
        Ok(())
    })
    .unwrap();
}

#[test]
#[ignore = "needs CRAFT_TEST_INSPYRENET_WEIGHTS and CRAFT_TEST_INSPYRENET_REFERENCE"]
fn matches_pytorch_on_the_processor() {
    #[allow(deprecated)]
    let cpu = burn::backend::ndarray::NdArrayDevice::Cpu;
    run(cpu.into());
}

#[test]
#[ignore = "needs CRAFT_TEST_INSPYRENET_WEIGHTS, CRAFT_TEST_INSPYRENET_REFERENCE and a graphics card"]
fn matches_pytorch_on_a_graphics_card() {
    run(WgpuDevice::default().into());
}
