"""PyTorch reference for the parity test in crates/ai-engine-burn/src/inspyrenet/parity.rs: the
input, every stage and the prediction of InSPyReNet Swin-B, run by transparent-background's own
code (pip install transparent-background torch safetensors) on a synthetic 384² picture.

usage: python tools/burn/inspyrenet_reference.py <ckpt_base.pth or inspyrenet.safetensors> <out.safetensors>
"""
import sys
import types

import torch
import torch.nn as nn

# The Swin code imports three helpers from timm; stand-ins keep timm (and its version) out of it.
layers = types.ModuleType("timm.models.layers")
layers.DropPath = lambda *a, **k: nn.Identity()
layers.to_2tuple = lambda x: x if isinstance(x, tuple) else (x, x)
layers.trunc_normal_ = lambda t, *a, **k: t
timm = types.ModuleType("timm")
models = types.ModuleType("timm.models")
models.layers = layers
timm.models = models
sys.modules.update({"timm": timm, "timm.models": models, "timm.models.layers": layers})

from safetensors.torch import save_file  # noqa: E402
from transparent_background.InSPyReNet import InSPyReNet_SwinB  # noqa: E402

ckpt, out = sys.argv[1], sys.argv[2]
model = InSPyReNet_SwinB(depth=64, pretrained=False, threshold=None, base_size=[1024, 1024])
if ckpt.endswith(".safetensors"):
    from safetensors.torch import load_file

    state = load_file(ckpt)
else:
    state = torch.load(ckpt, map_location="cpu", weights_only=True)
model.load_state_dict(state, strict=True)
model.eval()

n = 384
ys, xs = torch.meshgrid(torch.linspace(0, 1, n), torch.linspace(0, 1, n), indexing="ij")
disc = (((xs - 0.55) ** 2 + (ys - 0.5) ** 2) < 0.08).float()
g = torch.Generator().manual_seed(0)
rgb = torch.stack([0.2 + 0.6 * disc, 0.3 + 0.3 * ys, 0.7 - 0.4 * disc * xs]) + 0.03 * torch.randn(3, n, n, generator=g)
rgb = rgb.clamp(0, 1)
mean = torch.tensor([0.485, 0.456, 0.406]).view(3, 1, 1)
std = torch.tensor([0.229, 0.224, 0.225]).view(3, 1, 1)
x = ((rgb - mean) / std).unsqueeze(0).contiguous()

with torch.no_grad():
    backbone = model.backbone(x)
    o = model.forward_inspyre(x)
    pred = torch.sigmoid(o["saliency"][3])
    pred = (pred - pred.min()) / (pred.max() - pred.min() + 1e-8)

tensors = {"input": x, "pred": pred}
tensors.update({f"backbone.{i}": t for i, t in enumerate(backbone)})
tensors.update({f"saliency.{i}": t for i, t in enumerate(o["saliency"])})
tensors.update({f"laplacian.{i}": t for i, t in enumerate(o["laplacian"])})
save_file({k: v.float().contiguous() for k, v in tensors.items()}, out)
print({k: tuple(v.shape) for k, v in tensors.items()})
