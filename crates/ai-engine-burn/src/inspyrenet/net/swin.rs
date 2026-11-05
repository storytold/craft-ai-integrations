//! The Swin Transformer backbone (Z. Liu et al., "Swin Transformer: Hierarchical Vision
//! Transformer using Shifted Windows", ICCV 2021), in the detection variant InSPyReNet uses:
//! windows padded to the window size, a shift in every second block, a LayerNorm on each stage
//! output, and the patch embedding as an extra first output.

use burn::module::{Module, Param};
use burn::nn::Linear;
use burn::nn::conv::{Conv2d, Conv2dConfig};
use burn::tensor::activation::{gelu, softmax};
use burn::tensor::{Device, Int, Tensor, TensorData};

use super::nn::{LayerNorm, linear};

/// Swin-B hyperparameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwinConfig {
    pub embed_dim: usize,
    pub depths: [usize; 4],
    pub heads: [usize; 4],
    pub window: usize,
    pub patch: usize,
}

impl SwinConfig {
    /// Swin-B, window 12 (InSPyReNet's backbone).
    pub const BASE: Self = Self {
        embed_dim: 128,
        depths: [2, 2, 18, 2],
        heads: [4, 8, 16, 32],
        window: 12,
        patch: 4,
    };
}

#[derive(Module, Debug)]
pub struct WindowAttention {
    pub qkv: Linear,
    pub proj: Linear,
    /// `[(2·window − 1)², heads]`.
    pub relative_position_bias_table: Param<Tensor<2>>,
    pub heads: usize,
}

#[derive(Module, Debug)]
pub struct Mlp {
    pub fc1: Linear,
    pub fc2: Linear,
}

#[derive(Module, Debug)]
pub struct Block {
    pub norm1: LayerNorm,
    pub attn: WindowAttention,
    pub norm2: LayerNorm,
    pub mlp: Mlp,
}

#[derive(Module, Debug)]
pub struct PatchMerging {
    pub norm: LayerNorm,
    pub reduction: Linear,
}

#[derive(Module, Debug)]
pub struct Stage {
    pub blocks: Vec<Block>,
    pub downsample: Option<PatchMerging>,
}

#[derive(Module, Debug)]
pub struct PatchEmbed {
    pub proj: Conv2d,
    pub norm: LayerNorm,
}

#[derive(Module, Debug)]
pub struct Swin {
    pub patch_embed: PatchEmbed,
    pub layers: Vec<Stage>,
    pub norm0: LayerNorm,
    pub norm1: LayerNorm,
    pub norm2: LayerNorm,
    pub norm3: LayerNorm,
    pub window: usize,
}

impl Swin {
    pub fn new(cfg: SwinConfig, dev: &Device) -> Self {
        let w = cfg.window;
        let layers = (0..4)
            .map(|i| {
                let c = cfg.embed_dim << i;
                let heads = cfg.heads[i];
                let blocks = (0..cfg.depths[i])
                    .map(|_| Block {
                        norm1: LayerNorm::new(c, dev),
                        attn: WindowAttention {
                            qkv: linear(c, 3 * c, true, dev),
                            proj: linear(c, c, true, dev),
                            relative_position_bias_table: Param::from_tensor(Tensor::zeros(
                                [(2 * w - 1) * (2 * w - 1), heads],
                                dev,
                            )),
                            heads,
                        },
                        norm2: LayerNorm::new(c, dev),
                        mlp: Mlp {
                            fc1: linear(c, 4 * c, true, dev),
                            fc2: linear(4 * c, c, true, dev),
                        },
                    })
                    .collect();
                let downsample = (i < 3).then(|| PatchMerging {
                    norm: LayerNorm::new(4 * c, dev),
                    reduction: linear(4 * c, 2 * c, false, dev),
                });
                Stage { blocks, downsample }
            })
            .collect();
        let e = cfg.embed_dim;
        Self {
            patch_embed: PatchEmbed {
                proj: Conv2dConfig::new([3, e], [cfg.patch, cfg.patch])
                    .with_stride([cfg.patch, cfg.patch])
                    .init(dev),
                norm: LayerNorm::new(e, dev),
            },
            layers,
            norm0: LayerNorm::new(e, dev),
            norm1: LayerNorm::new(2 * e, dev),
            norm2: LayerNorm::new(4 * e, dev),
            norm3: LayerNorm::new(8 * e, dev),
            window: w,
        }
    }

    /// `x`: `[1, 3, H, W]` with `H` and `W` multiples of 32. Returns the patch embedding and the
    /// four normalised stage outputs, each `[1, C, h, w]`. `cancelled` is asked between stages;
    /// `None` when it said yes.
    pub fn forward(&self, x: Tensor<4>, cancelled: &dyn Fn() -> bool) -> Option<[Tensor<4>; 5]> {
        let dev = x.device();
        let x = self.patch_embed.proj.forward(x); // [b, C, h, w]
        let embed = self.patch_embed.norm.forward(x.permute([0, 2, 3, 1])); // [b, h, w, C]
        let first = embed.clone().permute([0, 3, 1, 2]);
        let rel = rel_index(self.window, &dev);
        let norms = [&self.norm0, &self.norm1, &self.norm2, &self.norm3];
        let mut outs = Vec::with_capacity(4);
        let mut x = embed;
        for (stage, norm) in self.layers.iter().zip(norms) {
            if cancelled() {
                return None;
            }
            let [_, sh, sw, _] = x.dims();
            let mask = shift_mask(self.window, sh, sw, &dev);
            for (i, blk) in stage.blocks.iter().enumerate() {
                let shift = if i % 2 == 1 { self.window / 2 } else { 0 };
                x = block(
                    blk,
                    x,
                    self.window,
                    shift,
                    &rel,
                    (shift > 0).then_some(&mask),
                );
            }
            outs.push(norm.forward(x.clone()).permute([0, 3, 1, 2]));
            if let Some(m) = &stage.downsample {
                x = merge(m, x);
            }
        }
        let mut it = outs.into_iter();
        Some([first, it.next()?, it.next()?, it.next()?, it.next()?])
    }
}

/// Relative position index of a `w × w` window, flattened to `[N·N]`.
fn rel_index(w: usize, dev: &Device) -> Tensor<1, Int> {
    let n = w * w;
    let wi = w as i64;
    let mut idx = Vec::with_capacity(n * n);
    for i in 0..n {
        for j in 0..n {
            let (hi, ci, hj, cj) = (
                (i / w) as i64,
                (i % w) as i64,
                (j / w) as i64,
                (j % w) as i64,
            );
            idx.push((hi - hj + wi - 1) * (2 * wi - 1) + (ci - cj + wi - 1));
        }
    }
    Tensor::from_data(TensorData::new(idx, [n * n]), dev)
}

fn padded(n: usize, w: usize) -> usize {
    n.div_ceil(w) * w
}

/// The shifted-window attention mask of an `h × w` grid: `[nW, N, N]`, −100 between tokens of
/// different regions.
fn shift_mask(win: usize, h: usize, w: usize, dev: &Device) -> Tensor<3> {
    let (hp, wp, s) = (padded(h, win), padded(w, win), win / 2);
    let region = |v: usize, len: usize| {
        if v + win < len {
            0
        } else if v + s < len {
            1
        } else {
            2
        }
    };
    let label = |y: usize, x: usize| region(y, hp) * 3 + region(x, wp);
    let (nwh, nww, n) = (hp / win, wp / win, win * win);
    let mut m = Vec::with_capacity(nwh * nww * n * n);
    for wy in 0..nwh {
        for wx in 0..nww {
            let lab: Vec<usize> = (0..n)
                .map(|t| label(wy * win + t / win, wx * win + t % win))
                .collect();
            for a in &lab {
                for b in &lab {
                    m.push(if a == b { 0.0f32 } else { -100.0 });
                }
            }
        }
    }
    Tensor::from_data(TensorData::new(m, [nwh * nww, n, n]), dev)
}

/// `[B, Hp, Wp, C]` → `[B·nW, win², C]`.
fn partition(x: Tensor<4>, win: usize) -> Tensor<3> {
    let [b, h, w, c] = x.dims();
    x.reshape([b, h / win, win, w / win, win, c])
        .permute([0, 1, 3, 2, 4, 5])
        .reshape([b * (h / win) * (w / win), win * win, c])
}

/// Inverse of [`partition`].
fn reverse(x: Tensor<3>, win: usize, b: usize, h: usize, w: usize) -> Tensor<4> {
    let c = x.dims()[2];
    x.reshape([b, h / win, w / win, win, win, c])
        .permute([0, 1, 3, 2, 4, 5])
        .reshape([b, h, w, c])
}

/// Zero-pads dims 1 and 2 of `[B, H, W, C]` at the bottom and right to multiples of `win`.
fn pad(x: Tensor<4>, win: usize) -> Tensor<4> {
    let [b, h, w, c] = x.dims();
    let dev = x.device();
    let (hp, wp) = (padded(h, win), padded(w, win));
    let x = if wp > w {
        Tensor::cat(vec![x, Tensor::zeros([b, h, wp - w, c], &dev)], 2)
    } else {
        x
    };
    if hp > h {
        Tensor::cat(vec![x, Tensor::zeros([b, hp - h, wp, c], &dev)], 1)
    } else {
        x
    }
}

fn block(
    blk: &Block,
    x: Tensor<4>,
    win: usize,
    shift: usize,
    rel: &Tensor<1, Int>,
    mask: Option<&Tensor<3>>,
) -> Tensor<4> {
    let [b, h, w, c] = x.dims();
    let heads = blk.attn.heads;
    let shortcut = x.clone();
    let mut y = pad(blk.norm1.forward(x), win);
    let (hp, wp) = (padded(h, win), padded(w, win));
    let s = shift as i64;
    if shift > 0 {
        y = y.roll(&[-s, -s], &[1, 2]);
    }
    let windows = partition(y, win);
    let [bw, n, _] = windows.dims();
    let d = c / heads;
    let qkv = blk
        .attn
        .qkv
        .forward(windows)
        .reshape([bw, n, 3, heads, d])
        .permute([2, 0, 3, 1, 4]);
    let take = |i: usize| qkv.clone().narrow(0, i, 1).reshape([bw, heads, n, d]);
    let (q, k, v) = (take(0) * (d as f64).powf(-0.5), take(1), take(2));
    let mut attn = q.matmul(k.swap_dims(2, 3));
    let bias = blk
        .attn
        .relative_position_bias_table
        .val()
        .select(0, rel.clone())
        .reshape([n, n, heads])
        .permute([2, 0, 1])
        .unsqueeze_dim::<4>(0);
    attn = attn + bias;
    if let Some(m) = mask {
        let nw = m.dims()[0];
        attn = (attn.reshape([bw / nw, nw, heads, n, n])
            + m.clone().unsqueeze_dim::<4>(1).unsqueeze_dim::<5>(0))
        .reshape([bw, heads, n, n]);
    }
    let out = softmax(attn, 3)
        .matmul(v)
        .swap_dims(1, 2)
        .reshape([bw, n, c]);
    let mut y = reverse(blk.attn.proj.forward(out), win, b, hp, wp);
    if shift > 0 {
        y = y.roll(&[s, s], &[1, 2]);
    }
    let x = shortcut + y.narrow(1, 0, h).narrow(2, 0, w);
    x.clone()
        + blk
            .mlp
            .fc2
            .forward(gelu(blk.mlp.fc1.forward(blk.norm2.forward(x))))
}

/// Patch merging: the 2 × 2 neighbours concatenated in PyTorch's order (`x[0::2, 0::2]`,
/// `x[1::2, 0::2]`, `x[0::2, 1::2]`, `x[1::2, 1::2]`), normalised and projected to 2C. Odd sizes
/// are zero-padded first.
fn merge(m: &PatchMerging, x: Tensor<4>) -> Tensor<4> {
    let [b, h, w, c] = x.dims();
    let dev = x.device();
    let x = if w % 2 == 1 {
        Tensor::cat(vec![x, Tensor::zeros([b, h, 1, c], &dev)], 2)
    } else {
        x
    };
    let x = if h % 2 == 1 {
        Tensor::cat(vec![x, Tensor::zeros([b, 1, w + w % 2, c], &dev)], 1)
    } else {
        x
    };
    let [b, h, w, c] = x.dims();
    let x = x
        .reshape([b, h / 2, 2, w / 2, 2, c])
        .permute([0, 1, 3, 4, 2, 5])
        .reshape([b, h / 2, w / 2, 4 * c]);
    m.reduction.forward(m.norm.forward(x))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn max_diff<const D: usize>(a: Tensor<D>, b: Tensor<D>) -> f32 {
        (a - b).abs().max().into_scalar::<f32>()
    }

    #[test]
    fn windows_and_rolls_are_exact_inverses() {
        let dev = Device::flex();
        let x = Tensor::<4>::random([1, 36, 48, 8], burn::tensor::Distribution::Default, &dev);
        assert_eq!(
            max_diff(x.clone(), reverse(partition(x.clone(), 12), 12, 1, 36, 48)),
            0.0
        );
        assert_eq!(
            max_diff(
                x.clone(),
                x.clone()
                    .roll(&[-6i64, -6], &[1, 2])
                    .roll(&[6i64, 6], &[1, 2])
            ),
            0.0
        );
    }

    #[test]
    fn merge_takes_neighbours_in_pytorch_order() {
        let dev = Device::flex();
        // A 2 × 2 grid of one channel holding 0, 1 (top row) and 2, 3 (bottom row).
        let x = Tensor::<4>::from_data(
            TensorData::new(vec![0.0f32, 1.0, 2.0, 3.0], [1, 2, 2, 1]),
            &dev,
        );
        let cat = x
            .reshape([1, 1, 2, 1, 2, 1])
            .permute([0, 1, 3, 4, 2, 5])
            .reshape([1, 1, 1, 4]);
        // x[0::2,0::2] = 0, x[1::2,0::2] = 2, x[0::2,1::2] = 1, x[1::2,1::2] = 3.
        assert_eq!(
            cat.into_data().try_to_vec::<f32>().unwrap(),
            [0.0, 2.0, 1.0, 3.0]
        );
    }

    #[test]
    fn shift_mask_regions_match_the_reference_slices() {
        // 24 × 24 with window 12, shift 6: the last window row and column hold three regions.
        let m = shift_mask(12, 24, 24, &Device::flex())
            .into_data()
            .try_to_vec::<f32>()
            .unwrap();
        let n = 144;
        // Window 0 is one region: no masking.
        assert!(m[..n * n].iter().all(|v| *v == 0.0));
        // Window 3 (bottom right) mixes four regions: its first and last token differ.
        let w3 = &m[3 * n * n..4 * n * n];
        assert_eq!(w3[n - 1], -100.0);
        assert_eq!(w3[0], 0.0);
    }
}
