# Deliberate-bug checks

Each case breaks one safety rule in the source, runs the one test that should notice, and puts the
source back. A case is **caught** only if the mutated code compiled and the test run failed. A mutant
that does not compile counts as not caught, so every survivor is either a test gap (fix the test) or a
bad mutant (fix the mutant). Cargo runs serially, targeted with `-p` and `--locked`,
honouring `CARGO_BUILD_JOBS` from the build resource.

| Script | Groups |
|---|---|
| `mutation_check.py` | `u1 u3 u4 p4 p10 v1 v2 v3 sd vx lc lr nr ct u2 wf` (connections, model sources, node packs, media, client flows, declarative adapter, workflow library). Cases for `u2` are in `mutation_u2.py`. |
| `mutation_check.py seg` | Shared selection contract: points, box, description and checked result metadata. |
| `mutation_check.py rt` | rten model runner, bounded picture decoding, resampling and prompt assembly. |
| `mutation_check.py rt` | rten engine picture handling: bounded PNG and JPEG decoding, mask encoding, resampling. |
| `mutation_check.py bp` | Local Burn weight preparation (`ai-burn-setup`): source hashes, cache key, damaged saved weights, cancel. |
| `mutation_check.py be` | Burn engine: saved-weights loading, the panic guard, answer checks, file names, its catalogue route, the `.bpk` binding rule and the shared architecture revision. |
| `mutation_check.py bo` | A recipe only built-in runtimes run is never offered, planned, installed, registered or advised as a ComfyUI model. |
| `mutation_check.py fr` | First-run setup flow: the licence tick only where needed, the one start button, the suggested folder, adopting checked files, the route-aware try-it sample, status and picker when a file has gone, route notes and engine advice. Cases in `mutation_first_run.py`. |
| `mutation_check.py gs` | Generic selection: scratch sinks, the refusal of clicks and mapped selection workflows. |
| `mutation_check.py gs` | Generic selection workflows: the scratch sinks, the refusal of clicks and mapped selection workflows. |
| `mutation_check.py gu` | Generic selection uploads: one upload per picture, the bounded memory, the resend after a cleared scratch area or a failed job. |
| `mutation_check.py gm` | Generic selection masks: size, one channel, the empty-mask answer and the picture header. |
| `mutation_check.py s21` | SAM 2.1 selection (`sam21_session.rs`, `detector.rs`): the Florence-2-large preflight, the detector name and the choice check. |
| `mutation_check.py ca` | Upstream contract alignment: model-test address provenance, required inputs, and app-facing refusal before key access. Cases in `mutation_contract.py`. |
| `mutation_check.py ps` | Node pack status report (`pack/status.rs`): links, hidden folders, names, the install record. |
| `mutation_check.py vx` | Vertex express adapter (`ai-vertex/src/lib.rs`): the API key travels in the `x-goog-api-key` header, never in the request URL. |
| `mutation_check.py lg` | Remembered licences (`registry/licences.rs`, `install.rs`): terms, versions, local files, damaged record. |
| `mutation_check.py eg` | Connection panel lifecycle (`ai-egui/src/panel.rs`): the same-frame Save dedup and the stale-answer generation guard. Cases in `mutation_egui.py`; receipt and logs in `verification/eg-mutations.json`. |
| `sam3_mutation_check.py` | `sg1 sg2 sg2b sg2c sg3` (SAM 3 masks, sessions, slots, cancel). |

```sh
python tools/mutation/mutation_check.py u2            # one group
python tools/mutation/mutation_check.py u2 stale      # cases whose name contains "stale"
```

The "Deliberate-bug checks" workflow runs a group on Ubuntu and macOS. It takes the group and an
optional case-name filter, so one case can be checked on both systems in minutes instead of waiting
for the group. A filter that names no case in the group fails instead of reporting success:

```text
group:  p4
filter: input-folder-read
```

Group ids are historical labels. The header of each test file says what it covers.

Run them exclusively. They snapshot and restore source files, so do not edit, format or build those
files while one runs. Afterwards read the restored source and rerun the focused tests.

Logs go to `tools/mutation/verification/` (not committed) after passing through `scrub.py`, which
replaces a Windows or Unix profile folder with `%USERPROFILE%` so no account name is written.
