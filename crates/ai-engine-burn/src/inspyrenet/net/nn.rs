//! Building blocks shared by the networks, named after their PyTorch counterparts so a PyTorch
//! checkpoint loads by key without renaming: `weight` / `bias` on the norms and PyTorch's `same`
//! padding on the convolutions. Linear layers use Burn's own `[in, out]` layout: Burn's PyTorch
//! reader transposes PyTorch's `[out, in]` weights as it loads them.

use burn::module::{Module, Param};
use burn::nn::conv::{Conv2d, Conv2dConfig};
use burn::nn::{Linear, LinearConfig, PaddingConfig2d};
use burn::tensor::activation::{relu, softmax};
use burn::tensor::module::{conv2d, interpolate};
use burn::tensor::ops::{ConvOptions, InterpolateMode, InterpolateOptions, PadMode};
use burn::tensor::{Device, Tensor, TensorData};

/// PyTorch's default normalisation epsilon.
const EPS: f64 = 1e-5;

/// A linear layer (Burn's `[in, out]` weight layout).
pub fn linear(d_in: usize, d_out: usize, bias: bool, dev: &Device) -> Linear {
    LinearConfig::new(d_in, d_out).with_bias(bias).init(dev)
}

/// `nn.LayerNorm` over the last dimension.
#[derive(Module, Debug)]
pub struct LayerNorm {
    pub weight: Param<Tensor<1>>,
    pub bias: Param<Tensor<1>>,
}

impl LayerNorm {
    pub fn new(d: usize, dev: &Device) -> Self {
        Self {
            weight: Param::from_tensor(Tensor::ones([d], dev)),
            bias: Param::from_tensor(Tensor::zeros([d], dev)),
        }
    }

    pub fn forward<const D: usize>(&self, x: Tensor<D>) -> Tensor<D> {
        let mean = x.clone().mean_dim(D - 1);
        let centred = x - mean;
        let var = centred.clone().square().mean_dim(D - 1);
        let norm = centred / (var + EPS).sqrt();
        let shape = |t: Tensor<1>| {
            let mut dims = [1usize; D];
            dims[D - 1] = t.dims()[0];
            t.reshape(dims)
        };
        norm * shape(self.weight.val()) + shape(self.bias.val())
    }
}

/// `nn.BatchNorm2d` in evaluation mode (running statistics).
#[derive(Module, Debug)]
pub struct BatchNorm {
    pub weight: Param<Tensor<1>>,
    pub bias: Param<Tensor<1>>,
    pub running_mean: Param<Tensor<1>>,
    pub running_var: Param<Tensor<1>>,
}

impl BatchNorm {
    pub fn new(c: usize, dev: &Device) -> Self {
        Self {
            weight: Param::from_tensor(Tensor::ones([c], dev)),
            bias: Param::from_tensor(Tensor::zeros([c], dev)),
            running_mean: Param::from_tensor(Tensor::zeros([c], dev)),
            running_var: Param::from_tensor(Tensor::ones([c], dev)),
        }
    }

    pub fn forward(&self, x: Tensor<4>) -> Tensor<4> {
        let c = self.weight.dims()[0];
        let scale = self.weight.val() / (self.running_var.val() + EPS).sqrt();
        let shift = self.bias.val() - self.running_mean.val() * scale.clone();
        x * scale.reshape([1, c, 1, 1]) + shift.reshape([1, c, 1, 1])
    }
}

/// InSPyReNet's `Conv2d` wrapper: a bias-free convolution with PyTorch `same` padding
/// (`dilation · (k − 1) / 2` per side), then batch norm and ReLU when enabled.
#[derive(Module, Debug)]
pub struct ConvBn {
    pub conv: Conv2d,
    pub bn: Option<BatchNorm>,
    pub relu: bool,
}

impl ConvBn {
    pub fn new(
        c_in: usize,
        c_out: usize,
        kernel: [usize; 2],
        dilation: usize,
        bn: bool,
        relu: bool,
        dev: &Device,
    ) -> Self {
        let (ph, pw) = (
            dilation * (kernel[0] - 1) / 2,
            dilation * (kernel[1] - 1) / 2,
        );
        let conv = Conv2dConfig::new([c_in, c_out], kernel)
            .with_dilation([dilation, dilation])
            .with_padding(PaddingConfig2d::Explicit(ph, pw, ph, pw))
            .with_bias(false)
            .init(dev);
        Self {
            conv,
            bn: bn.then(|| BatchNorm::new(c_out, dev)),
            relu,
        }
    }

    /// A square `k × k` convolution with batch norm.
    pub fn square(c_in: usize, c_out: usize, k: usize, relu: bool, dev: &Device) -> Self {
        Self::new(c_in, c_out, [k, k], 1, true, relu, dev)
    }

    pub fn forward(&self, x: Tensor<4>) -> Tensor<4> {
        let mut x = self.conv.forward(x);
        if let Some(bn) = &self.bn {
            x = bn.forward(x);
        }
        if self.relu { relu(x) } else { x }
    }
}

/// InSPyReNet's axial `SelfAttention`. Its query, key and value maps are *reshaped* (not
/// transposed) to `(b, −1, axis)`, which mixes rows and columns in memory order; the port does
/// the same so it computes what the trained weights expect.
#[derive(Module, Debug)]
pub struct SelfAttention {
    pub query_conv: ConvBn,
    pub key_conv: ConvBn,
    pub value_conv: ConvBn,
    pub gamma: Param<Tensor<1>>,
    /// Attend along the height (`"h"`) rather than the width (`"w"`).
    pub along_height: bool,
}

impl SelfAttention {
    pub fn new(c: usize, along_height: bool, dev: &Device) -> Self {
        Self {
            query_conv: ConvBn::new(c, c / 8, [1, 1], 1, true, false, dev),
            key_conv: ConvBn::new(c, c / 8, [1, 1], 1, true, false, dev),
            value_conv: ConvBn::new(c, c, [1, 1], 1, true, false, dev),
            gamma: Param::from_tensor(Tensor::zeros([1], dev)),
            along_height,
        }
    }

    pub fn forward(&self, x: Tensor<4>) -> Tensor<4> {
        let [b, c, h, w] = x.dims();
        let axis = if self.along_height { h } else { w };
        let flat = |t: Tensor<4>| {
            let [_, tc, th, tw] = t.dims();
            t.reshape([b, tc * th * tw / axis, axis])
        };
        let query = flat(self.query_conv.forward(x.clone())).swap_dims(1, 2);
        let key = flat(self.key_conv.forward(x.clone()));
        let attention = softmax(query.matmul(key), 2);
        let value = flat(self.value_conv.forward(x.clone()));
        let out = value
            .matmul(attention.swap_dims(1, 2))
            .reshape([b, c, h, w]);
        out * self.gamma.val().reshape([1, 1, 1, 1]) + x
    }
}

/// Bilinear resize of `[b, c, h, w]` to `size`, with PyTorch's `align_corners` convention.
pub fn resize(x: Tensor<4>, size: [usize; 2], align_corners: bool) -> Tensor<4> {
    let [_, _, h, w] = x.dims();
    if [h, w] == size {
        // The identity under either convention.
        return x;
    }
    interpolate(
        x,
        InterpolateOptions::new(InterpolateMode::Bilinear)
            .with_align_corners(align_corners)
            .with_output_size(size),
    )
}

/// `F.pixel_shuffle`: `[b, c·r², h, w]` → `[b, c, h·r, w·r]`.
pub fn pixel_shuffle(x: Tensor<4>, r: usize) -> Tensor<4> {
    let [b, crr, h, w] = x.dims();
    let c = crr / (r * r);
    x.reshape([b, c, r, r, h, w])
        .permute([0, 1, 4, 2, 5, 3])
        .reshape([b, c, h * r, w * r])
}

/// The Laplacian image pyramid InSPyReNet reconstructs its saliency with: a 7 × 7 Gaussian with
/// σ = 1 (OpenCV's `getGaussianKernel`, normalised), on one channel.
pub struct ImagePyramid {
    kernel: Tensor<4>,
}

impl ImagePyramid {
    pub const KSIZE: usize = 7;

    pub fn new(dev: &Device) -> Self {
        let k = Self::KSIZE;
        let g: Vec<f64> = (0..k)
            .map(|i| (-((i as f64 - (k - 1) as f64 / 2.0).powi(2)) / 2.0).exp())
            .collect();
        let sum: f64 = g.iter().sum();
        let g: Vec<f64> = g.iter().map(|v| v / sum).collect();
        let outer: Vec<f32> = g
            .iter()
            .flat_map(|a| g.iter().map(move |b| (a * b) as f32))
            .collect();
        Self {
            kernel: Tensor::from_data(TensorData::new(outer, [1, 1, k, k]), dev),
        }
    }

    /// Doubles the size: each sample lands on the even pixel of its 2 × 2 cell, then the
    /// Gaussian (×4, for the three zeros) fills the rest.
    pub fn expand(&self, x: Tensor<4>) -> Tensor<4> {
        let z = Tensor::zeros_like(&x);
        let x = pixel_shuffle(Tensor::cat(vec![x, z.clone(), z.clone(), z], 1), 2);
        let p = Self::KSIZE / 2;
        let x = x.pad([(p, p), (p, p)], PadMode::Reflect);
        conv2d(
            x,
            self.kernel.clone() * 4.0,
            None,
            ConvOptions::new([1, 1], [0, 0], [1, 1], 1),
        )
    }

    /// The next finer level: `x` expanded plus the Laplacian detail.
    pub fn reconstruct(&self, x: Tensor<4>, laplacian: Tensor<4>) -> Tensor<4> {
        let expanded = self.expand(x);
        let [_, _, h, w] = expanded.dims();
        // PyTorch resizes the detail to the expanded size (align_corners = True); at equal
        // sizes that is the identity.
        let laplacian = if laplacian.dims()[2..] == [h, w] {
            laplacian
        } else {
            resize(laplacian, [h, w], true)
        };
        expanded + laplacian
    }
}
