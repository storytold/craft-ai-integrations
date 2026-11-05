# The Burn engine (SAM 2.1 and InSPyReNet, optional)

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
| The catalogue route | `crates/ai-features/src/catalogue/sam21.rs`: a `burn` route after `rten`, same two published files |
| The cargo feature | `craft-ai-client` feature `burn` |

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
# Through the client, two apps
cargo test -p craft-ai-client --features burn --test burn_engine -j 8 -- --ignored
python tools/mutation/mutation_check.py be
```

## Limits to know

- SAM 2.1 tiny only. A different architecture needs its own generated code and runner.
- Click and box prompts. A description stays on the ComfyUI route.
- The processor fallback uses Burn's `ndarray` backend, which Burn has marked deprecated in favour
  of `burn-flex`. Swap it once flex is measured. It is built with Burn's `rayon` and `simd`
  features: the crate turns Burn's default features off, and without these two the processor runs
  on one core with scalar maths (3.7 times slower on the one network measured, see the pull request
  that turned them on). The SAM 2.1 timings below were taken before that change.
- `CRAFT_AI_BURN_DEVICE=cpu` forces the processor even when a graphics card answers. Run on the
  test PC (release, astronaut sample): one click and one box both gave IoU 1.0000 against rten
  (22.5 % selected by both). Timings on the processor: first run 8.70 s (rten 0.71 s), second
  click on the same picture 1.78 s (rten 0.030 s). Usable, but the card is the point of this engine.
- A few generated `forward` functions are 500 to 699 lines because a function cannot be split
  without tracking every value that passes between the halves. No file is 700 lines or more.
- Progress and Cancel during the first preparation: `load` has neither, so the app shows its own
  busy state until the first selection returns.

## InSPyReNet for `image.matte`

The second model on this engine: `inspyrenet` (engine `burn`), the subject's soft alpha for
Remove background, from the picture alone. It is not SAM: no clicks, no box, no prompt.

| Piece | Where |
|---|---|
| Runner, request and answer | `src/inspyrenet/mod.rs`: a PNG or JPEG `source` in, one grayscale PNG at the picture's size out (white is the subject, soft along hair and fur) |
| The network | `src/inspyrenet/net/`: InSPyReNet with the Swin-B backbone (T. Kim et al., ACCV 2022), **written by hand** in Burn after the MIT-licensed reference code (plemeri/InSPyReNet, as packaged by transparent-background). Module names follow the PyTorch keys, so the checkpoint loads by name |
| Weights | `src/inspyrenet/weights.rs`: the `.safetensors` checkpoint read with Burn's PyTorch adapter (linear weights transposed as they load); a load that leaves any weight unset, fails one, or finds a tensor this network does not have is refused in plain words |
| Any picture size | `src/inspyrenet/pipeline/`: full resolution up to 1280 pixels on the long side (reflect-padded to a multiple of 32); above that, reflect-padded to a square, reduced with antialiased Lanczos-3 to 1280², the matte enlarged back and, below half scale, refined at full resolution with a colour guided filter (He et al., PAMI 2013) on the tiles near the edge; then a 0.2 fringe shrink. The approach of the Gyre background remover |

Unlike SAM 2.1 here, nothing is generated and nothing is prepared on the computer: the runner
reads the checkpoint file as downloaded.

### The weights file

`inspyrenet.safetensors` from [1038lab/inspyrenet](https://huggingface.co/1038lab/inspyrenet) at
revision `0f5946638dcbc7dbc1e56306b823959958ac0ae0`: 367,120,444 bytes, SHA-256
`5c8ab4541f9a960b376a80362b07f1d96262e689c501365e6323faa958de47ea`. It is a format conversion
with no model card of its own. On 29/10/2026 it was checked tensor for tensor (all 1,625 identical)
against the author's own checkpoint, `ckpt_base.pth` (InSPyReNet Swin-B Plus Ultra, MIT) at
[plemeri/InSPyReNet](https://huggingface.co/plemeri/InSPyReNet) revision
`d94c2baaa4d023ab018c6f97be6ef37548e3bd1f`: 367,520,613 bytes, SHA-256
`0a6fe2a73ab0532d6d0b8d82849a9760a226df719e3063d09b4149ece6f80fcd`, the file transparent-background
downloads. The author's file is not used directly because it is a pickle (`.pth`), and a local
connection takes only `.safetensors`, `.onnx` or `.bpk` weights.

### In the catalogue

`remove-background` has one recipe, InSPyReNet (`crates/ai-features/src/catalogue/inspyrenet.rs`),
and it is **built in only**: `LocalRecipe::comfyui` is false, so it is never a ComfyUI setup option,
plan, install offer or connection, and engine advice does not offer ComfyUI for the tool. Its one
route is `burn` / `inspyrenet` with the pinned file above in the folder `inspyrenet`, so Set up AI
offers one 367 MB download, checked by size and SHA-256, and records that file on the connection.
A node pack and a workflow would be far more to set up for one file and one tool.

### What was run

On one Windows machine (i5-12600KF, RTX 3090 Ti), release:

- **Parity with PyTorch** (`src/inspyrenet/parity.rs`): every stage of the network (patch
  embedding, four Swin stages, the saliency and Laplacian pyramids, the prediction) against
  transparent-background's own code on a synthetic 384² input. Worst relative difference over the
  stages 3.2e-5 on the processor and 2.4e-4 on the graphics card; no pixel of the prediction
  changes side at 0.5.
- **The shared conformance suite** (`check_runner`) on the runner's request handling and pipeline
  around a stand-in for the network, as the SAM 2.1 tests here do. The network itself, a tiny one
  saved as `.bpk`, runs through load and run in an ignored test on the processor, in release (about
  4 s). Unoptimised, Burn's processor backend takes minutes for it, and a build server's software
  renderer is too slow and, on Linux (lavapipe), fails.
- **The whole runner on a real photo**, against the same port inside PhotoCraft (which runs it
  through the same pipeline): on a 3101 × 1613 PNG the two masks are identical, byte for byte. On a
  JPEG they differ by 0.003 on average (IoU 0.994), from the two JPEG decoders.
- **Through the shared client** (`crates/ai-client/tests/burn_matte.rs`): the launcher sets up the
  catalogue's built-in route with the real file, then two app ids ask `AiClient` for Remove
  background only and each gets one grayscale mask at the picture's size (a 2848 × 4272 PNG). A
  file that is not the checkpoint is refused in words that point to Set up AI.
- Not measured cleanly: the time per picture. During these runs another program held about 20 GB of
  the card's memory and the PhotoCraft port slowed down just as much (about 20 s a picture instead
  of about 2.5 s). No run on macOS or Linux, and no real app has mounted it yet.

```sh
# The PyTorch reference, once (Python with torch, safetensors and transparent-background)
python tools/burn/inspyrenet_reference.py <inspyrenet.safetensors> <inspyrenet-ref-384.safetensors>
# Then the parity and weight tests, release
CRAFT_TEST_INSPYRENET_WEIGHTS=<inspyrenet.safetensors> CRAFT_TEST_INSPYRENET_REFERENCE=<inspyrenet-ref-384.safetensors>   cargo test --release -p craft-ai-engine-burn -j 8 --lib inspyrenet -- --include-ignored --nocapture
# One photo through the runner, the mask written out
CRAFT_TEST_INSPYRENET_WEIGHTS=<inspyrenet.safetensors> CRAFT_TEST_INSPYRENET_IMAGE=<photo> CRAFT_TEST_INSPYRENET_OUT=<mask.png>   cargo test --release -p craft-ai-engine-burn -j 8 --lib removes_the_background -- --ignored --nocapture
# Two apps through the shared client, from the catalogue's route
CRAFT_TEST_INSPYRENET_WEIGHTS=<inspyrenet.safetensors> CRAFT_TEST_INSPYRENET_IMAGE=<photo.png>   cargo test --release -p craft-ai-client --features burn --test burn_matte -- --ignored --nocapture
# The tiny network through load and run, on the processor
cargo test --release -p craft-ai-engine-burn --lib tiny_network -- --ignored
python tools/mutation/mutation_check.py be inspyrenet
python tools/mutation/mutation_check.py bo
```

### Limits to know

- **Graphics card advice.** The route is not marked quick on the processor, so engine advice
  calls it slow on a computer without a card the engine can use.
- **A software renderer counts as a card.** The engine takes the graphics card when one answers,
  and a software renderer (lavapipe, WARP) answers too. On CI the network was far slower there and
  failed on lavapipe. So the first failure on a card that is not a cancel builds the network again
  on the processor from the same file and runs that picture again there with a notice. Once that
  rebuild has worked, the model stays on the processor. The move is tried once: if the rebuild
  fails, its error is the answer and later pictures go to the card again. A load on the card that
  fails is likewise retried on the processor, unless the file itself is not the checkpoint
  (`src/inspyrenet/fallback.rs`). SAM 2.1 on this engine makes the same choice of
  device without that move. `CRAFT_AI_BURN_DEVICE=cpu` forces the processor from the start.
- **The processor is slow.** The fallback runs on Burn's `ndarray` backend, which this crate
  builds without its `multi-threads` and `simd` features, so it uses one core. The same network on
  one core took about 230 s a picture in PhotoCraft (its `flex` backend), about 35 s with four
  threads and SIMD. The graphics card is the point of this runner.
- The parity reference is made by `tools/burn/inspyrenet_reference.py`, which needs Python with
  PyTorch and transparent-background.
