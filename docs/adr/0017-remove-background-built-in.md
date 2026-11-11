# ADR 0017: Remove background runs built in, with InSPyReNet on Burn

- **Status:** accepted 30/10/2026
- **Source:** [#112](https://github.com/storytold/craft-ai-integrations/pull/112)

## Context

Remove background had no local model, and every local recipe was also a ComfyUI model. The tool has to be easy to reach, without ComfyUI, node packs, workflows or Python.

## Decision

`craft-ai-engine-burn` gains an `inspyrenet` runner (InSPyReNet Swin-B, MIT reference code) that answers `image.matte` from a picture alone, built in only. It was chosen over four BiRefNet v2 variants after the comparison in storytold/photocraft#4098, and moved into the shared layer from a PhotoCraft-only crate.

## Consequences

Any app with the tool gets a background remover with no extra setup. A built-in route no longer has to be a ComfyUI recipe.
