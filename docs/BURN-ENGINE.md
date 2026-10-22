# The Burn engine (SAM 2.1, optional)

`craft-ai-engine-burn` runs SAM 2.1 tiny for `image.segment` through Burn, on a graphics card when
one answers and on the processor when none does. It is a choice beside rten, never the default:
rten reads the publisher's ONNX files as they are and stays first in the recipe.

The pieces, and which crate owns each:

| Piece | Where |
|---|---|
| Request handling (clicks, box, the picture kept between requests, empty-mask answer, size checks) | `craft-ai-engine-rten` (`Loaded` over a `Backend`), shared by both engines |
| The model run on Burn, weight loading, the panic guard | `craft-ai-engine-burn` |
| The compiled architecture (generated) | `crates/ai-engine-burn/src/model/` |
| One-time local conversion of the published ONNX files to saved weights | `craft-ai-burn-setup` (`docs/LOCAL-BURN-PREPARATION.md`) |
| The catalogue route (planned, the next change) | `crates/ai-features/src/catalogue/sam21.rs`: a `burn` route after `rten`, same two published files |
| The cargo feature (planned, the next change) | `craft-ai-client` feature `burn` |

Nothing converted is hosted. The saved weights (`encoder.bpk`, `decoder.bpk`) are made on the
person's computer, once, and shared by every app. Given the downloaded `encoder.onnx` (with
`decoder.onnx` beside it), the runner prepares the saved copy itself: it calls the setup crate,
which checks the exact source bytes, then writes `burn-prepared/` beside the model folder (never
inside it) and loads from there. Later loads only check hashes. The converter cannot be
interrupted and `load` has no Cancel handle, so the first load runs to the end (seconds on the
test PC, to be measured on a clean machine). Any other `.onnx` is refused in plain words. The
`prepare` cargo feature of `craft-ai-engine-burn` (on by default) bundles the converter; with it
off, a downloaded file is answered with "open Set up AI".

## Regenerating the architecture

The generated code is never edited by hand. It comes from one script, so a reviewer can rerun it:

```sh
python tools/burn/generate_sam21.py <folder holding encoder.onnx and decoder.onnx>
python tools/burn/generate_sam21.py <folder> --check    # exit 1 if the tree is not what it makes
```

The script checks the two files against the sizes and SHA-256 pinned in `craft-ai-burn-setup`,
runs the pinned `burn-onnx` (an example of that crate), applies five fixes to its output (each must
match exactly the stated number of times, or the script stops), drops the generated loaders that
panic on a bad file, splits by submodule so every file is under 700 lines, runs rustfmt and writes
`src/model/`. The five fixes, all found on SAM 2.1 tiny with burn-onnx 0.22.0:

1. An integer scalar divided by an Int tensor was emitted as a float divide (once, encoder).
2. `one_hot` output rank was not inferred (once, decoder).
3. A zero-length integer `ConstantOfShape` (Hiera's window padding is 0 when the size is a
   multiple of the window), which Burn's `expand` refuses (five, encoder).
4. The same for the float one, the decoder's empty box placeholder (three, decoder).
5. `Where` with a condition that broadcasts: Burn's `mask_where` wants it at full size (five,
   decoder).

They are worth reporting upstream to `tracel-ai/burn-onnx`.

### The revision rule

`REVISION` (in the script) names the compiled architecture and the converter that made its weights.
It is part of the saved-weights cache key in `crates/ai-burn-setup/src/sam21.rs`. Change it in both
places, and regenerate, whenever any of these changes: the `burn-onnx` or Burn version, a patch in
the script, or the source ONNX files. Saved weights made for an older revision are then never reused.
`crates/ai-engine-burn/tests/revision.rs` fails if the script, the generated `model/mod.rs` and the
cache key disagree.

## Exercising it

```sh
# Unit tests with a fake backend (no model, no graphics card needed)
cargo test -p craft-ai-engine-burn -j 8
# Prepare the weights once, then the live comparison with rten (release; skipped without the variables)
cargo run -p craft-ai-burn-setup --features conversion --example prepare_sam21 -- <onnx dir> <cache dir>
CRAFT_TEST_SAM21_ONNX_DIR=<onnx dir> CRAFT_TEST_SAM21_BURN_DIR=<folder printed above> \
CRAFT_TEST_SAM21_IMAGE=<512 square sample> \
  cargo test --release -p craft-ai-engine-burn --test live -j 8 -- --ignored --nocapture
python tools/mutation/mutation_check.py be
```

## Limits to know

- SAM 2.1 tiny only. A different architecture needs its own generated code and runner.
- Click and box prompts. A description stays on the ComfyUI route.
- The processor fallback uses Burn's `ndarray` backend, which Burn has marked deprecated in favour
  of `burn-flex`. Swap it once flex is measured.
- `CRAFT_AI_BURN_DEVICE=cpu` forces the processor even when a graphics card answers. Run on the
  test PC (release, astronaut sample): one click and one box both gave IoU 1.0000 against rten
  (22.5 % selected by both). Timings on the processor: first run 8.70 s (rten 0.71 s), second
  click on the same picture 1.78 s (rten 0.030 s). Usable, but the card is the point of this engine.
- A few generated `forward` functions are 500 to 699 lines because a function cannot be split
  without tracking every value that passes between the halves. No file is 700 lines or more.
- Progress and Cancel during the first preparation: `load` has neither, so the app shows its own
  busy state until the first selection returns.
