//! InSPyReNet (T. Kim, K. Kim, J. Lee, D. Cha, J. Lee and D. Kim, "Revisiting Image Pyramid
//! Structure for High Resolution Salient Object Detection", ACCV 2022), ported from the
//! MIT-licensed reference implementation (github.com/plemeri/InSPyReNet, as packaged by
//! transparent-background). Module and field names follow the PyTorch code, so its checkpoints
//! load by key.
//!
//! The network predicts a saliency pyramid: a coarse map from the decoder at 1/8 resolution,
//! refined twice by scale-invariant context attention (SICA) through a Laplacian pyramid up to
//! the input size.

#![allow(non_snake_case)] // `Hattn` / `Wattn`, as named in the checkpoints

use burn::module::{Module, Param};
use burn::tensor::activation::{relu, sigmoid, softmax};
use burn::tensor::{Device, Tensor};

pub mod nn;
pub mod swin;

use nn::{ConvBn, ImagePyramid, SelfAttention, resize};
use swin::{Swin, SwinConfig};

/// Channels of the decoder and attention features.
pub const DEPTH: usize = 64;
/// The size SICA pools its context at, at stage 0 (InSPyReNet's `base_size`); stage `s` uses
/// `BASE_SIZE >> s`.
pub const BASE_SIZE: usize = 1024;

/// `PAA_kernel`: a 1 × 1 projection, a 1 × r and an r × 1 convolution, axial attention along
/// both axes, then a 3 × 3 convolution dilated by r.
#[derive(Module, Debug)]
pub struct PaaKernel {
    pub conv0: ConvBn,
    pub conv1: ConvBn,
    pub conv2: ConvBn,
    pub conv3: ConvBn,
    pub Hattn: SelfAttention,
    pub Wattn: SelfAttention,
}

impl PaaKernel {
    fn new(c_in: usize, c_out: usize, r: usize, dev: &Device) -> Self {
        Self {
            conv0: ConvBn::new(c_in, c_out, [1, 1], 1, true, false, dev),
            conv1: ConvBn::new(c_out, c_out, [1, r], 1, true, false, dev),
            conv2: ConvBn::new(c_out, c_out, [r, 1], 1, true, false, dev),
            conv3: ConvBn::new(c_out, c_out, [3, 3], r, true, false, dev),
            Hattn: SelfAttention::new(c_out, true, dev),
            Wattn: SelfAttention::new(c_out, false, dev),
        }
    }

    fn forward(&self, x: Tensor<4>) -> Tensor<4> {
        let x = self
            .conv2
            .forward(self.conv1.forward(self.conv0.forward(x)));
        self.conv3
            .forward(self.Hattn.forward(x.clone()) + self.Wattn.forward(x))
    }
}

/// `PAA_e`: the context module on each backbone output.
#[derive(Module, Debug)]
pub struct PaaE {
    pub branch0: ConvBn,
    pub branch1: PaaKernel,
    pub branch2: PaaKernel,
    pub branch3: PaaKernel,
    pub conv_cat: ConvBn,
    pub conv_res: ConvBn,
}

impl PaaE {
    fn new(c_in: usize, c_out: usize, dev: &Device) -> Self {
        Self {
            branch0: ConvBn::new(c_in, c_out, [1, 1], 1, true, false, dev),
            branch1: PaaKernel::new(c_in, c_out, 3, dev),
            branch2: PaaKernel::new(c_in, c_out, 5, dev),
            branch3: PaaKernel::new(c_in, c_out, 7, dev),
            conv_cat: ConvBn::square(4 * c_out, c_out, 3, false, dev),
            conv_res: ConvBn::new(c_in, c_out, [1, 1], 1, true, false, dev),
        }
    }

    fn forward(&self, x: Tensor<4>) -> Tensor<4> {
        let cat = Tensor::cat(
            vec![
                self.branch0.forward(x.clone()),
                self.branch1.forward(x.clone()),
                self.branch2.forward(x.clone()),
                self.branch3.forward(x.clone()),
            ],
            1,
        );
        relu(self.conv_cat.forward(cat) + self.conv_res.forward(x))
    }
}

/// `PAA_d`: the decoder that turns the three deepest context features into the coarse map.
#[derive(Module, Debug)]
pub struct PaaD {
    pub conv1: ConvBn,
    pub conv2: ConvBn,
    pub conv3: ConvBn,
    pub conv4: ConvBn,
    pub conv5: ConvBn,
    pub Hattn: SelfAttention,
    pub Wattn: SelfAttention,
}

impl PaaD {
    fn new(c_in: usize, depth: usize, dev: &Device) -> Self {
        Self {
            conv1: ConvBn::square(c_in, depth, 3, false, dev),
            conv2: ConvBn::square(depth, depth, 3, false, dev),
            conv3: ConvBn::square(depth, depth, 3, false, dev),
            conv4: ConvBn::square(depth, depth, 3, false, dev),
            conv5: ConvBn::new(depth, 1, [3, 3], 1, false, false, dev),
            Hattn: SelfAttention::new(depth, true, dev),
            Wattn: SelfAttention::new(depth, false, dev),
        }
    }

    /// `(features, coarse map)` from `[x3, x4, x5]` (finest first), at `x3`'s size.
    fn forward(&self, x3: Tensor<4>, x4: Tensor<4>, x5: Tensor<4>) -> (Tensor<4>, Tensor<4>) {
        let [_, _, h, w] = x3.dims();
        let fx = Tensor::cat(
            vec![resize(x5, [h, w], true), resize(x4, [h, w], true), x3],
            1,
        );
        let fx = self.conv1.forward(fx);
        let fx = self
            .conv2
            .forward(self.Hattn.forward(fx.clone()) + self.Wattn.forward(fx));
        let fx = self.conv4.forward(self.conv3.forward(fx));
        let out = self.conv5.forward(fx.clone());
        (fx, out)
    }
}

/// `SICA`: scale-invariant context attention. It splits the coarser map into foreground,
/// background and uncertain areas (plus the Laplacian's, with `lmap_in`), pools one context
/// vector per area at a fixed size, and lets every pixel attend to them.
#[derive(Module, Debug)]
pub struct Sica {
    pub conv_query: Vec<ConvBn>,
    pub conv_key: Vec<ConvBn>,
    pub conv_value: Vec<ConvBn>,
    pub conv_out1: ConvBn,
    pub conv_out2: ConvBn,
    pub conv_out3: ConvBn,
    pub conv_out4: ConvBn,
    pub threshold: Param<Tensor<1>>,
    pub lthreshold: Option<Param<Tensor<1>>>,
    pub depth: usize,
    /// The side the context is pooled at.
    pub stage_size: usize,
}

impl Sica {
    fn new(c_in: usize, depth: usize, stage: usize, lmap_in: bool, dev: &Device) -> Self {
        let pair = |k: usize| {
            vec![
                ConvBn::square(c_in, depth, k, true, dev),
                ConvBn::square(depth, depth, k, true, dev),
            ]
        };
        let half = || Param::from_tensor(Tensor::from_floats([0.5], dev));
        Self {
            conv_query: pair(3),
            conv_key: pair(1),
            conv_value: pair(1),
            conv_out1: ConvBn::square(depth, depth, 3, true, dev),
            conv_out2: ConvBn::square(c_in + depth, depth, 3, true, dev),
            conv_out3: ConvBn::square(depth, depth, 3, true, dev),
            conv_out4: ConvBn::square(depth, 1, 1, false, dev),
            threshold: half(),
            lthreshold: lmap_in.then(half),
            depth,
            stage_size: BASE_SIZE >> stage,
        }
    }

    fn seq(convs: &[ConvBn], x: Tensor<4>) -> Tensor<4> {
        convs.iter().fold(x, |x, c| c.forward(x))
    }

    /// `(features, Laplacian detail)` for `x`, guided by the coarser saliency `smap` and, with
    /// `lmap_in`, the coarser Laplacian `lmap`.
    fn forward(
        &self,
        x: Tensor<4>,
        smap: Tensor<4>,
        lmap: Option<Tensor<4>>,
    ) -> (Tensor<4>, Tensor<4>) {
        let [b, c, h, w] = x.dims();
        let t = self.threshold.val().reshape([1, 1, 1, 1]);
        let p = sigmoid(resize(smap, [h, w], false)) - t.clone();
        let mut prob = vec![
            p.clone().clamp(0.0, 1.0),
            p.clone().neg().clamp(0.0, 1.0),
            t - p.abs(),
        ];
        if let (Some(lt), Some(lmap)) = (&self.lthreshold, lmap) {
            let lp = sigmoid(resize(lmap, [h, w], false)) - lt.val().reshape([1, 1, 1, 1]);
            prob.push(lp.clone().clamp(0.0, 1.0));
            prob.push(lp.neg().clamp(0.0, 1.0));
        }
        let ctx = prob.len();
        let (s, d) = (self.stage_size, self.depth);
        // PyTorch views these as (b, s², −1) and (b, ctx, s²): reshapes in memory order.
        let f = resize(x.clone(), [s, s], false).reshape([b, s * s, c]);
        let prob = resize(Tensor::cat(prob, 1), [s, s], false).reshape([b, ctx, s * s]);
        let context = prob.matmul(f).swap_dims(1, 2).unsqueeze_dim::<4>(3); // [b, c, ctx, 1]
        let query = Self::seq(&self.conv_query, x.clone())
            .reshape([b, d, h * w])
            .swap_dims(1, 2);
        let key = Self::seq(&self.conv_key, context.clone()).reshape([b, d, ctx]);
        let value = Self::seq(&self.conv_value, context)
            .reshape([b, d, ctx])
            .swap_dims(1, 2);
        let sim = softmax(query.matmul(key) * (d as f64).powf(-0.5), 2);
        let context = self
            .conv_out1
            .forward(sim.matmul(value).swap_dims(1, 2).reshape([b, d, h, w]));
        let x = self
            .conv_out3
            .forward(self.conv_out2.forward(Tensor::cat(vec![x, context], 1)));
        let out = self.conv_out4.forward(x.clone());
        (x, out)
    }
}

/// Everything the network computes on the way, for parity tests against the reference. Only the
/// finest saliency is used outside them.
pub struct Parts {
    /// The patch embedding and the four Swin stages.
    #[cfg_attr(not(test), allow(dead_code))]
    pub backbone: [Tensor<4>; 5],
    /// Saliency logits, coarse (1/8) to fine (full size).
    pub saliency: [Tensor<4>; 4],
    /// Laplacian details at 1/4, 1/2 and full size.
    #[cfg_attr(not(test), allow(dead_code))]
    pub laplacian: [Tensor<4>; 3],
}

/// InSPyReNet with the Swin-B backbone (`InSPyReNet_SwinB`).
#[derive(Module, Debug)]
pub struct InSPyReNet {
    pub backbone: Swin,
    pub context1: PaaE,
    pub context2: PaaE,
    pub context3: PaaE,
    pub context4: PaaE,
    pub context5: PaaE,
    pub decoder: PaaD,
    pub attention0: Sica,
    pub attention1: Sica,
    pub attention2: Sica,
}

impl InSPyReNet {
    /// The Swin-B architecture with placeholder weights (load a checkpoint before use).
    #[cfg(test)]
    pub fn new(dev: &Device) -> Self {
        Self::with_backbone(SwinConfig::BASE, dev)
    }

    /// The same network over a Swin backbone of another size (tests use a tiny one).
    pub fn with_backbone(cfg: SwinConfig, dev: &Device) -> Self {
        let e = cfg.embed_dim;
        Self {
            backbone: Swin::new(cfg, dev),
            context1: PaaE::new(e, DEPTH, dev),
            context2: PaaE::new(e, DEPTH, dev),
            context3: PaaE::new(2 * e, DEPTH, dev),
            context4: PaaE::new(4 * e, DEPTH, dev),
            context5: PaaE::new(8 * e, DEPTH, dev),
            decoder: PaaD::new(3 * DEPTH, DEPTH, dev),
            attention0: Sica::new(DEPTH, DEPTH, 0, true, dev),
            attention1: Sica::new(2 * DEPTH, DEPTH, 1, true, dev),
            attention2: Sica::new(2 * DEPTH, DEPTH, 2, false, dev),
        }
    }

    /// The saliency pyramid of a normalised `[1, 3, H, W]` input (`H`, `W` multiples of 32):
    /// PyTorch's `forward_inspyre`. `None` once `cancelled` says yes (asked between stages).
    pub fn forward_parts(&self, x: Tensor<4>, cancelled: &dyn Fn() -> bool) -> Option<Parts> {
        let [_, _, h, w] = x.dims();
        let pyramid = ImagePyramid::new(&x.device());
        let backbone = self.backbone.forward(x, cancelled)?;
        if cancelled() {
            return None;
        }
        let [b1, b2, b3, b4, b5] = backbone.clone();
        let x1 = self.context1.forward(b1);
        let x2 = self.context2.forward(b2);
        let x3 = self.context3.forward(b3);
        let x4 = self.context4.forward(b4);
        let x5 = self.context5.forward(b5);
        if cancelled() {
            return None;
        }
        let (f3, d3) = self.decoder.forward(x3, x4, x5);

        let f3 = resize(f3, [h / 4, w / 4], false);
        let (f2, p2) = self
            .attention2
            .forward(Tensor::cat(vec![x2, f3], 1), d3.clone(), None);
        let d2 = pyramid.reconstruct(d3.clone(), p2.clone());
        if cancelled() {
            return None;
        }

        let x1 = resize(x1, [h / 2, w / 2], false);
        let f2 = resize(f2, [h / 2, w / 2], false);
        let (f1, p1) =
            self.attention1
                .forward(Tensor::cat(vec![x1, f2], 1), d2.clone(), Some(p2.clone()));
        let d1 = pyramid.reconstruct(d2.clone(), p1.clone());
        if cancelled() {
            return None;
        }

        let f1 = resize(f1, [h, w], false);
        let (_, p0) = self.attention0.forward(f1, d1.clone(), Some(p1.clone()));
        let d0 = pyramid.reconstruct(d1.clone(), p0.clone());
        Some(Parts {
            backbone,
            saliency: [d3, d2, d1, d0],
            laplacian: [p2, p1, p0],
        })
    }

    /// Foreground probability `[1, 1, H, W]`: the finest saliency through a sigmoid, stretched to
    /// `0..=1` (PyTorch's `forward_inference` without the LR/HR blending, as Gyre runs it).
    pub fn forward(&self, x: Tensor<4>, cancelled: &dyn Fn() -> bool) -> Option<Tensor<4>> {
        let parts = self.forward_parts(x, cancelled)?;
        let [_, _, _, d0] = parts.saliency;
        let pred = sigmoid(d0);
        let (lo, hi) = (pred.clone().min(), pred.clone().max());
        let range = hi - lo.clone() + 1e-8;
        Some((pred - lo.reshape([1, 1, 1, 1])) / range.reshape([1, 1, 1, 1]))
    }
}
