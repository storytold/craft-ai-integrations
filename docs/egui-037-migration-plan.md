# egui 0.37 migration plan for craft-ai-integrations

Current publication candidate (2026-10-25): implementation and qualification changes are committed at `087f5a0` on the migration branch; the uncommitted-source descriptions below are historical. The current panel source, snapshot-test source and all five golden image hashes match the retained passing mutation-qualification packet. The changed widget package passed 108 tests, strict all-target lint and formatting; the separate consumer smoke passed a locked check and one draw test. Full remote workspace/platform CI, native host/IME/clipboard and paired performance acceptance remain open. Publication requires explicit approval; human review and human merge are required.

## Historical implementation and qualification record

Status: implementation and headless qualification are complete for the current follow-up;
**not acceptance-complete**. The source audit at
`1a02afd6bed425bcd516d8de2f3dc3b29614b23d` is retained below as historical evidence. The current
checkout is `7c80bbc52cb5a84cebeb67529a18d670a15c511b` plus local AI-5 follow-up changes (the
deterministic snapshot suite, the separate consumer-smoke root, the panel mutation group and these
docs). The follow-up passed 108 tests, strict all-target clippy and fmt on Linux; its five candidate
snapshots, lifecycle mutants and locked consumer smoke are checked, and an independent test-packet
review passed. Native GUI/IME, physical-GPU and performance qualification remain open. No push, pull
request, comment, release, merge or other publication is authorised; each future publication and the
final merge still need fresh explicit human approval after the exact content and validation are shown.

The repository is `storytold/craft-ai-integrations`. Its only egui consumer is `crates/ai-egui` (package `craft-ai-egui`).
This plan covers that crate, its examples, tests, workspace dependency graph, and the contracts it must keep for any app that
may adopt it.

### Current implemented state (2026-10-24)

| Item | Actual state |
| --- | --- |
| Source | `7c80bbc52cb5a84cebeb67529a18d670a15c511b`, plus the inherited pin-only root `Cargo.toml`/`Cargo.lock` updates and the local follow-up files listed in the change review. |
| Production egui family | Reviewed storytold `storytold/egui` at `6d518fc799681571963842f95b55a6b68431b15a`; one family source in the root lock. |
| Production winit | Reviewed storytold `storytold/winit` at `a517b3846ee8457d84ee4975110bfda6797c44dc`. |
| Root lock | `Cargo.lock` SHA-256 `7505336b3831762c90b73610723161c163852edfee43daea350630e93dd4d799`; retained unchanged by this documentation, snapshot and smoke-root work. |
| Consumer recipe | `consumer-smoke/` is a separate workspace that repeats the complete `[patch.crates-io]` recipe, path-depends on this repository's `ai-egui` and `ai-core`, and asserts the public widget values are that `craft_ai_core` identity. Its locked check and one draw test passed with one `craft-ai-core` identity and the pinned egui family; the rendererless root leaves the winit patches unused. |
| Font policy | The library keeps `egui` at `default-features = false`. Tests/examples opt into bundled `default_fonts`; the snapshot suite installs `FontDefinitions::default()` explicitly and keeps kittest's missing-glyph panic. A host supplies fonts or gets them through eframe; production does not force them. |
| Test-only features | `egui_kittest` uses `snapshot` + `wgpu` only as a dev dependency. `eframe` stays a dev dependency with `default_fonts`, `glow`, `x11`, `wayland`. No new public feature flag. |
| Duplicate core policy | This workspace and the smoke root use one path `craft-ai-core`. Do not combine it with `craft-libs/crates/ai-core`; replacing the local copy remains separate `CORE-SYNC` work. |
| Final headless qualification | Under managed job `egui037-ai-regression-mutation-20261024-2205`, **108 tests passed** with strict `clippy --all-targets` and `cargo fmt --all --check` green on Linux, then again after restoration. Five committed kittest snapshots are hash-stable. The `eg` group caught both compiled lifecycle mutants; changing the background from RGB(27,27,27) to RGB(30,30,30) failed the expected snapshot with 75,486 changed pixels. The locked consumer smoke passed one draw test with one `craft-ai-core` identity and the pinned egui family. An independent test-packet review passed. This is not evidence for native GUI, IME/clipboard, physical-GPU or performance acceptance. |
| Still open | Native Linux X11/Wayland and Windows widget workflows (focus, keyboard, clipboard/IME, screenshots); macOS IME/Metal/termination remains an explicit blocker; physical-GPU and performance qualification; any app adoption; human review of this documentation and fresh explicit approval for publication. The non-widget mutation groups were not rerun in the final headless job. |

## 1. Baseline and provenance

### 1.1 Exact source and dependency baseline

| Item | Recorded value |
| --- | --- |
| repository / checkout | `storytold/craft-ai-integrations` at `/home/codex/work/CraftApps/egui037-migration-20261022/craft-ai-integrations` |
| baseline commit / branch | `1a02afd6bed425bcd516d8de2f3dc3b29614b23d` (`feat(client): offer SAM 2.1 on Burn as an optional engine after rten (#91)`) / `codex/egui-037-plan-20261022` |
| workspace manifest | `Cargo.toml`, resolver 3, members `crates/*`, edition 2024, workspace MSRV 1.90 |
| `craft-ai-egui` MSRV | `rust-version = "1.95"` (`crates/ai-egui/Cargo.toml:9`) |
| lockfile | `Cargo.lock`, format 4, 943 packages |
| current egui family | `egui`, `eframe`, `egui_kittest`, `egui-winit`, `egui-wgpu`, `egui_glow`, `epaint`, `epaint_default_fonts`, `ecolor`, `emath` all 0.36.2 from crates.io |
| current access/kittest | `accesskit` 0.24.1, `accesskit_consumer` 0.35.0, `kittest` 0.4.0 |
| current winit / checksum | 0.30.13 / `a6755fa58a9f8350bd1e472d4c3fcc25f824ec358933bba33306d0b63df5978d` |
| current `egui` checksum | `dc938cc27cd911415e1e4d151bc56ff9df063d8db081374e87aeb805ea74b2ba` |
| upstream target / comparison | annotated tag `0.37.0` peeled commit `841a328ba2dd8f6996beb7189ebb7fc00e668a2a`; `0.36.2` peeled commit `49682f8baa058bf49e011035cfbd6e825f88a5ef` |
| candidate egui | `aiongg/egui` `craft/0.37` at `a4f9a17f64a6bae61350fb85804c9df0d9865916`; research input only |
| candidate winit | `aiongg/winit` `craft/0.30.13-lang` at `8b35388bd719e8cc86a95971ba0bc026c9a8f058`; research input only |
| published winit base | `storytold/winit` `craft/0.30.13` at `ffae5198770d7730e718043f035a772b7dbc3b1b` |
| shared contract baseline | `storytold/craft-libs` at `49190884278f16c9a9f5823d7698a8ffb5deb062` |
| existing shared UI work | `storytold/craft-libs` PR 199 head `b68de8c5f4f1c060904d211a7b1320aa0f7bce78`; reference only |

The annotated tag object SHA is not the peeled commit SHA. Every future manifest and lock entry must use the peeled commit or
the reviewed storytold fork commit, never a tag object, branch name, or `main`. The candidate branch names in the local
checkouts (`ai-0.37`/`ai-lang`) differ from the provenance names above; only the SHAs are accepted pins.

### 1.2 Current package targets and entry points

`crates/ai-egui/Cargo.toml` declares one library package and no `[[bin]]`, feature table, `[patch]`, or vendored source. The
workspace has no root examples directory; the two examples belong to the package.

- library target: `crates/ai-egui/src/lib.rs`
- `examples/picker_demo.rs`: native eframe window, sample-only store
- `examples/history_demo.rs`: native eframe window, sample log by default, optional real history read
- integration tests: `consent_terms.rs`, `history_view.rs`, `native_setup_view.rs`, `panel.rs`, `picker_view.rs`,
  `popup_view.rs`, `widgets.rs`
- unit tests inside `text.rs`, `history_text.rs`, `form.rs`
- deliberate-bug harness: `tools/mutation/`, groups `ce` (consent), `nw` (native setup), `v2` (picker), `v3` (popup)

Normal dependencies are local path dependencies: `craft-ai-core`, `craft-ai-client`, `craft-ai-features`
(`crates/ai-egui/Cargo.toml:12-14`) and `egui = "0.36"` (`:15`). Dev dependencies are `egui_kittest = "0.36"` (`:18`),
`eframe = { version = "0.36", default-features = false, features = ["default_fonts", "glow", "x11", "wayland"] }` (`:19`),
`craft-ai-native`, `craft-ai-features`, `tempfile`, `serde_json` (`:20-23`).

There is no optional `egui` feature. `craft-ai-client` has optional `candle`, `rten` and `burn` features, but `ai-egui` does
not enable them. Native-setup tests remove `candle` from `ProviderFactory::standard()` and register a fake adapter, so they
never exercise a real engine download or inference.

### 1.3 Consumers and the duplicate-core constraint

Inspection of all 15 app workspaces in the planning tree, including every `Cargo.toml` and `.rs` file under `crates/`,
`apps/`, and `xtask/`, found **no app dependency on `craft-ai-egui`, no import of `craft_ai_egui`, and no app manifest that
depends on any `craft-ai-*` package**.

| Consumer | Egui state in its current checkout | `craft-ai-egui` dependency |
| --- | --- | --- |
| CADCraft `1977a493...` | direct `egui`/`eframe`/`egui_kittest` 0.37; `craft-ui` at 0.36 revision `5c874d5...`; two egui versions in lock | none |
| SoundCraft `52471b37...` | direct 0.37; `craft-ui` at `5c874d5...`; two egui versions in lock | none |
| DesignCraft, EffectCraft, FilmCraft, LightCraft, PdfCraft, PhotoCraft, VectorCraft | 0.36.2; `craft-ui` `5c874d5...` (PdfCraft also vendors egui-wgpu/winit pieces; VectorCraft also vendors windows-link) | none |
| GridCraft, WordCraft | 0.36.2; `craft-ui` `d27646a5...` | none |
| DeckCraft, FigCraft | 0.36.2; no `craft-ui` | none |
| Craft Launcher | 0.36.2; own launcher UI | none |
| ArtCraft Native `800217ed...` (repository root) | 0.36.2; `eframe`, `egui_extras`, `kittest` | none |

`docs/LAYOUT.md` says PhotoCraft and the launcher “reach into it by path today”. That is not reproduced in the exact
checkouts above. Treat the sentence as stale or as describing an external branch; it must not be used as evidence of a shared
widget consumer. Any future adoption is a separate integration with its own plan, dependency pin, font policy and UI tests.

**Duplicate core is a future-adoption constraint, not an egui deliverable.** `craft-ai-egui` uses the local `craft-ai-core`
path (`crates/ai-egui/Cargo.toml:12`; package at `crates/ai-core/Cargo.toml:2`), while `craft-libs` contains another crate
with the same package name (`craft-libs/crates/ai-core/Cargo.toml:2`). `docs/CORE-SYNC.md:3-5,28-33` already records that the
copies are not interchangeable and lists contract additions that have not moved upstream. An app that adopted both would have
two package identities and distinct public types (`ConnectionId`, `DraftError`, …), and `cargo tree -d` would show the
duplicate.

The migration therefore does not merge the two implementations and does not switch the local core to a craft-libs pin as a
side effect. The AI-2 consumer-smoke workspace must prove the intended identity instead: it path-depends on `crates/ai-egui`
and this workspace's `crates/ai-core`, resolves exactly one `craft-ai-core` package in `cargo tree -d`, and includes a
type-level assertion that the widget-facing `ConnectionDraft`, `ConnectionId` and `DraftError` values are the same core
types. Switching to one authoritative core needs separate `CORE-SYNC` work, its own review, old-store round-trip tests and a
pinned upstream revision; it is not part of the egui migration.

## 2. What actually changes in egui 0.37

These are released 0.37 behavior, not fork behavior. The table separates source-confirmed changes from risks that need fresh
measurement.

| Change | Concrete impact here | Source anchor |
| --- | --- | --- |
| `default_fonts` is no longer an egui default | Current `egui = "0.36"` silently turns on bundled fonts. With 0.37 the same string would stop bundling them; `default-features = false` records intent but is not proof of the resolved graph. | `crates/egui/Cargo.toml` 0.37.0 `[features]`; PR #8583 |
| `WidgetType` is replaced by `accesskit::Role`; input widgets need accessible names | `connection_form`’s unlabelled Name field fails kittest’s new accessibility check; every input name/role must be explicit. | `crates/egui/src/lib.rs:642-657`; `crates/egui_kittest/src/accessibility.rs`; PRs #8555, #8589 |
| `Response::labelled_by`, `Ui::label_inputs_by`, `Ui::name_inputs` | Use the visible heading `Response::labelled_by(heading.id)` or `ui.label_inputs_by(...)`; `TextEdit` has `id_salt`/`id_source` but no `labelled_by` builder. | candidate `crates/egui/src/response.rs:1065-1074`, `ui.rs:1001-1034`; PRs #8604, #8608 |
| `Id::new`/`AsId` deprecated; `Ui::id` renamed `scope_id` | This crate does not call `Id::new`, `Ui::id`, or `AsId`. Existing `ComboBox::from_id_salt` and `ScrollArea::id_salt` remain right. Add salts for text edits and list scroll areas. | `crates/egui/src/id.rs`; `crates/egui/src/ui.rs`; PRs #8534, #8542, #8586 |
| `ComboBox` is atom-based; `selectable_label` takes `IntoAtoms` | `picker::model_row` still has a `LayoutJob -> WidgetText -> Atom` path, but multi-line layout and selected styling must be revalidated, not assumed identical. | `combo_box.rs`; `atomics/atom_kind.rs`; PR #8618 |
| Style defaults are roomier | Fixed `ROW_HEIGHT = 20.0` (`history.rs:38`) and the fixed `90.0` reserve (`progress.rs:33`) can clip or crowd after 0.37. Recompute from style or verify bounds. | `style.rs`; PRs #8617, #8445 |
| `Context::run_ui`/`run_pass`/`root_ui`/`run_logic` | All widgets already take `&mut Ui`; keep that. Never call `Context::root_ui` from inside `App::ui`/`run_ui` (documented panic). | `context.rs:950-1055`, `:1174-1202`; PR #8535 |
| Screenshot is callback-based | No screenshot request exists here. Do not add a generic screenshot bridge or helper. | `data/screenshot_callback.rs`; PR #8489 |
| System font provider / missing-glyph policy | Production keeps default tofu behavior rather than panic. Tests panic on missing glyphs and install fonts explicitly. | `egui_kittest/src/builder.rs:39-51,88-101`; PRs #8512, #8601 |
| PRIMARY/clipboard features | No clipboard calls, `arboard`, or PRIMARY reads exist in `ai-egui`. TextEdit clipboard behavior is host-inherited; this crate needs no clipboard adapter. | `egui-winit/src/clipboard.rs`; PR #8428 |
| Text tessellation #8731 / DX12 #8736 | Text-heavy fixtures may expose #8731 as slower tessellation, not wrong output. `ai-egui` owns no wgpu renderer; #8736 belongs to an app/eframe. | upstream #8731, #8736 (external reports) |
| TextEdit click-away #8763 / PR #8764 | `ConnectionPanel` does not read a TextEdit selection from a button action, so no in-crate workaround. PR #8764 is **open/unmerged**; 0.37.0 does not include the fix. | upstream #8763; unmerged #8764 |
| Hidden parent/child #8748 | No viewport, docking, or child-window code here. #8748 is closed/unmerged and is not behavior in 0.37.0. | upstream #8748 |

## 3. Fork, craft-libs PR 199, vendoring, and publication

### 3.1 Suite fork and winit prerequisites

The candidate fork (`aiongg/egui` `craft/0.37` at `a4f9a17f...`) is based on upstream 0.37.0 and contains E1–E21, including
E4 Wayland clipboard, E6/E21 IME, E7/E19 keyboard layouts and E21 delete-surrounding-bytes. The fork workspace patches winit
by a movable branch; Cargo does not propagate a dependency’s `[patch]` section.

**Final approved reachable storytold revisions are prerequisites, and their SHAs cannot be invented before
implementation/publication.** The production graph must pin the reviewed storytold egui family and winit by full immutable
commits that contain at least:

- **W15P native PRIMARY** and separate **E4/P2b**: E4 regular clipboard plus the winit-owned per-seat PRIMARY path on the
  existing connection/event queue. W15P/P2b accept a fresh pointer or keyboard triggering serial for the focused seat, claim
  synchronously while fresh, and serve only asynchronously (total 2 s/16 MiB, 4 readers/2 writers/queue 8, live
  owner-specific waker, canceled/stale results discarded). Egui records viewport/focus/input generation and pastes once only
  if the target is still current.
- **NEW E21b/P7b IME state — see the complete shared contract:** authoritative detail is egui plan E21b, winit plan W27 and
  the suite rollout's E21b/W27 contract; this crate only consumes built-in `TextEdit`. Non-negotiables: owner/
  context generation even for `Unavailable`; empty `Available` distinct and sent as an explicit empty payload; committed
  disable/enable clears caches on owner/unavailable transition; latest completed pass including `None` is authoritative;
  current-epoch `done`-serial mismatch applies incoming edits once and defers outgoing until match, obsolete epochs
  invalidate; new bounded payload allocates no whole document. Built-in `TextEdit` emits logical UTF-8 byte cursor/anchor
  plus remapped preedit; egui-winit sends it through W27 on Wayland only. New prerequisite, not existing E21/W27 behavior.
- The companion winit W15/W26/W27 behavior; E4 and E21 pair with them and must not be taken alone.

The atomic production unit is the complete P8 egui revision (P2b, P7 held-key pairing, P7b/E21b, P7c selected #8763/#8748
fixes, justified performance fixes) plus the completed winit revision (W15P, W27 lifecycle). No stock/intermediate pin.

`ai-egui` uses only standard widgets (`Ui`, `TextEdit`, `ComboBox`, `ScrollArea`, `Label`, `Button`, `ProgressBar`,
`RichText`, `LayoutJob`). It calls no fork-only API such as `Style::right_to_left`, `RawInput::latin_layout_chars`,
`ImeEvent::DeleteSurroundingBytes`, or `Context::set_language`. Therefore its source stays stock-compatible, but its
production manifest must resolve the coherent fork family. Built-in TextEdit users inherit the E21b producer; this crate has
no custom IME producer, so it does not add a model/preedit/byte-index adapter or set `None` placeholders. Those adapters
belong to the eight custom editors in the consumer apps.

### 3.2 craft-libs PR 199

PR 199 is useful prior art for the `WidgetType` to `accesskit::Role` mapping, `Id::new` to `Id::unique` / `Ui::id` to
`Ui::scope_id` renames, and kittest’s missing-glyph panic including the `⋮` fixture problem. It does not apply mechanically
here: `ai-egui` does not depend on `craft-ui`, has no `Id::new`/`Ui::id` calls, no `⋮` fixture or icon font, and its package
is `publish = false`.

**Do not merge its stale trees.** PR 199 head `b68de8c` (and the historical audit tree `ec8840a`) is stale relative to
`4919088`: it lacks current `craft-platform` and reverts security, settings, docking and printer fixes. Reuse only the
API-port hunks from `8614e4a04fae516fba16c22b31ff1008cb6be10b` and README changes from `1764bca` on current `4919088`,
preserving intervening fixes and provenance. Neither tree qualifies the resulting source. Use PR 199 only as evidence of API
renames and harness changes; do not copy its lockfile, merge commit, or unrelated fixture edits.

### 3.3 Vendors, patches, and package identity

Inventory of this checkout: no `vendor/` directory; no `[patch.crates-io]` or other `[patch]` in root or crate manifests; no
git dependencies; `Cargo.lock` contains the egui family at registry 0.36.2 and exactly one winit 0.30.13. `egui-wgpu` appears
in the lock, but the selected dev feature set is `glow`; lock presence is not proof that a wgpu backend is active.

There is no vendor fix to preserve or remove here. Do not add unrelated vendor changes. PdfCraft and VectorCraft vendors are
not dependencies of this crate and are handled in their own plans.

All workspace packages are `publish = false`. Publication reachability therefore means:

- app consumers pin this repository by immutable git revision (or a local path only in private development), not a crates.io
  version;
- every consumer root repeats the egui/winit `[patch]` entries, because patches do not propagate;
- no machine-local path patch, branch, or tag object may appear in a shipped manifest;
- a consumer-smoke workspace proves the exact patch recipe, because this workspace’s patch cannot be inherited by consumers.

## 4. Required implementation changes

No change below is authorized yet; this is the reviewable design.

### 4.1 Manifest, lockfile, and feature policy

The production dependency becomes the approved storytold egui revision, with an explicit font policy. A template (the `rev`
values are filled only from the published approval, never invented):

```toml
[dependencies]
egui = { git = "https://github.com/storytold/egui", rev = "<approved-storytold-egui>", default-features = false }
```

The root and the consumer-smoke root need `[patch.crates-io]` entries for every family package that actually resolves:
`egui`, `epaint`, `ecolor`, `emath`, `epaint_default_fonts`, `eframe`, `egui-winit`, `egui_glow`, `egui-wgpu`,
`egui_kittest`, plus `winit`. Include `egui-wgpu` and `egui_kittest` because the mandatory pixel suite uses
`egui_kittest/wgpu`; include `egui_glow` because the examples use eframe’s glow backend. Do not patch or add packages the
root does not resolve.

Policy:

- Keep `eframe` as a dev dependency for examples with the same non-default 0.37 features: `default_fonts`, `glow`, `x11`,
  `wayland`. Do not add `wgpu`, `persistence`, `accesskit` or `system_fonts`; each is a separate capability decision.
- Keep `egui_kittest` as a dev dependency and add `snapshot` + `wgpu` for the pixel suite. Its normal `egui` dependency
  inherits `default-features = false` (`crates/egui_kittest/Cargo.toml:39-42`); only kittest's own `[dev-dependencies]`
  enable `default_fonts`/`monochrome_emoji_fonts` (`:65-68`), which do not propagate downstream. This test root gets fonts
  from its own `eframe` dev dependency (`crates/eframe/Cargo.toml:56` → `egui/default_fonts`); otherwise add that dev feature.
- Keep `craft-ai-egui` at `rust-version = "1.95"`. The workspace remains 1.90 for non-egui crates; egui 0.37’s MSRV is 1.95.
- No new public feature flags. Optional engine features remain on `craft-ai-client`, not here.

**Resolve features per root; a direct `default-features = false` alone proves nothing.** Run `cargo tree -e features` and
`cargo metadata` for the production library, the test/example root and the consumer-smoke root. The production graph must
not silently enable `egui/default_fonts` through eframe, another member or feature unification; tests/examples use bundled
fonts and the smoke root declares its own policy. A consumer may enable fonts through eframe or its own provider. Add a
host-font/no-tofu test and do not treat the direct flag as resolved-graph evidence.

Lockfile acceptance:

- `Cargo.lock` regenerated by Cargo under a managed `task.py run` job, never edited by hand.
- Exactly one source for `egui`, `epaint`, `ecolor`, `emath`, `epaint_default_fonts`, `eframe`, `egui-winit`, `egui-wgpu`,
  `egui_glow`, and `egui_kittest`; exactly one `winit`.
- No registry egui copy beside the selected git copy. Package IDs must use the approved storytold revision, not an aiongg
  branch. `cargo tree -d`, `cargo tree -i`, and `cargo metadata` results are review evidence.

### 4.2 Accessible names, roles, and stable IDs

Current break under 0.37:

- `form.rs:185-186` draws a `TextEdit` for the connection name with no hint text and no label association; kittest treats it
  as an unnamed `TextInput` and fails.
- Address and Key have placeholder hints, so they would be named by placeholder, but that is a weak, mutable name; the
  visible headings are not associated.
- `form.rs:150` uses a preset `ComboBox` whose selected text is a value, not a stable field name.
- `history.rs:136,166` and `status.rs:89` use combos whose selected values or label-derived salts are not stable field names.

Refactor:

1. Make the `heading` helper return `egui::Response`. Use the actual API
   `ui.add(TextEdit::singleline(...).id_salt(...)).labelled_by(heading.id)` for Name, Address and Key, or wrap the field in
   `ui.label_inputs_by(heading.id, |ui| ...)`. Do **not** write `TextEdit::labelled_by`: `TextEdit` has no such builder
   method.
2. Add explicit `.id_salt(...)` to the three TextEdits: `"craft-ai-connection-name"`, `"craft-ai-connection-address"`,
   `"craft-ai-connection-key"`. This keeps focus/selection on the same field across row insertion or translation.
3. Give the preset combo a visible `Start from` label response and call `.labelled_by` on `show_ui(...).response`. For
   history/status combos use `.accessible_name(...)`: “Filter by app”, “Filter by model”, “Date range”, and a `view.label`-based “model” name for the status picker.
4. Add a stable salt to the history scroll area. `AsIdSalt` requires `Hash + Debug` (candidate
   `crates/egui/src/id_salt.rs:5-9`), and `Tab` currently derives only `Clone, Copy, Debug, Default, PartialEq, Eq`
   (`history.rs:74`). Either add `Hash` to `Tab` and use `.id_salt(("history-list", state.tab))`, or add an explicit stable
   enum key and use `.id_salt(("history-list", state.tab.key()))`. Do not depend on `Debug` formatting. The grouped lists get
   a stable `"history-list"`/tab salt; `picker` already uses `("craft-ai-picker", view.tool)`.
5. Do not change the public `StatusView` shape. The label-based ID remains a documented limitation; a later localization PR
   may add a stable role ID with its own API decision and migration note.
6. Assert roles and names for every input: `TextInput`, `PasswordInput`, `ComboBox`, `CheckBox`, `Button`, `RadioButton`;
   check disabled and selected state. Do not disable `HarnessBuilder::with_accessibility_check` globally.

Compatibility contract: public signatures/action enums remain unchanged; accessible names do not change pixels; explicit ID
salts change widget identity relative to old auto IDs, which is acceptable because the package has no current app consumers.
Document it and keep the salts stable.

### 4.3 ConnectionPanel lifecycle: logic/UI split and once-only side effects

Current path (`panel.rs:63-121`) does everything inside `show(&mut Ui)`: polls `TestRunner`, sets the form’s test line,
requests a 100 ms repaint, starts a network test on `FormEvent::Test`, and writes the shared store on `FormEvent::Save`. That
is three side-effect classes in a UI callback. Under the pinned 0.36/0.37 eframe integration, **both native and web call
`App::logic` and `App::ui` inside the `run_ui` closure, and both can call logic once per layout pass**. After a normally
completed `run_ui`, `current_pass_index()` is zero; `end_pass` takes the output and `run_dyn` restores only before the next
pass. Visibility is not a phase: `RequestPaintWhileHidden` can run UI while minimized/occluded, and a false `visible()` does not prove a logic-only callback. A first-pass guard is therefore not a hidden-work fix; prefer consumed
queues, generation identity and monotonic deadlines, and test command/side-effect outcomes.

Refactor to an explicit command queue:

```rust
enum PanelCommand {
    StartTest { target: String, draft: ConnectionDraft, generation: u64 },
    Save(ConnectionDraft),
}

pub struct ConnectionPanel {
    manager: ConnectionManager,
    runner: TestRunner,
    form: ConnectionFormState,
    testing: Option<String>,
    generation: u64,
    pending: Option<PanelCommand>,
}
pub fn poll(&mut self, ctx: &egui::Context) -> Option<PanelEvent>;
pub fn ui(&mut self, ui: &mut Ui) -> Option<PanelEvent>;
pub fn show(&mut self, ui: &mut Ui) -> Option<PanelEvent>; // compatibility: poll then ui
```

Behavior:

- `ui` draws only. On Test it records one `StartTest` command, **sets `testing` to the captured endpoint immediately**, sets
  `form.test = Running`, clears the old error, and calls `ui.ctx().request_repaint()`. Setting `testing` immediately is
  required: the current form disables Test when `testing` is `Some` (`form.rs:250-253`), so a second UI pass in the same
  frame sees the button disabled and cannot enqueue a second start. `pending` is an additional admission guard. `ui` does not
  spawn a thread.
- On Save, `ui` records one `Save` with a captured draft snapshot, requests a repaint so the next poll runs, and does not
  write the store. On Cancel it returns `Closed`, which `show` queues after any pending `Saved`.
- `poll` takes `&egui::Context` so it can request a repaint after 100 ms while a test is active even from hidden logic. It
  drains at most one pending command, then polls `TestRunner` once. It starts the runner exactly once. It updates the form
  only when the captured endpoint and generation still match the current form, preserving the provenance check
  (`panel.rs:65-92`).
- Editing the address increments the generation and cancels/invalidates the in-flight result; a late answer for a changed
  endpoint or canceled generation is dropped and leaves the form untested.
- `poll` returns `PanelEvent::Saved` only after `manager.save` succeeds; it stores the error in the form otherwise. `Closed`
  remains UI-returned.
- `show` calls `poll(ui.ctx())` first, then `ui`. Events enter a FIFO `VecDeque<PanelEvent>`; `show` returns one per call,
  queueing `Saved` before `Closed` when both occur in one frame, so a host that keeps calling until `Closed`/empty loses
  neither. Click actions execute on the next poll/frame, which is acceptable for an interactive loop and makes repeated
  passes idempotent. Tests call `Harness::run`/`step` so the next frame occurs.
- Every app that wants correct hidden-window behavior calls `panel.poll(ctx)` from `App::logic` and `panel.ui(ui)` from
  `App::ui`; `show` remains for simple/UI-only hosts.
- Do not put `ConnectionPanel` state in `egui::Memory` or a `Context` bridge. It is an app-owned object passed to both hooks.

Examples get the same discipline:

- `picker_demo` calls `ToolDesk::choose` in `App::ui` (`examples/picker_demo.rs:254-269`). Queue `PickerAction`/`PopupAction`
  in `pending` and drain it in `App::logic` once; `refresh()` stays in logic.
- `history_demo` writes `history-export.csv` in `App::ui` (`examples/history_demo.rs:196-205`). Queue `HistoryAction::Export`
  and write in logic; `Clear` only clears the displayed log and stays local.
- `native_model_setup` already uses `SetupSession`; `begin` refuses when busy. The host still queues the returned
  `NativeSetupAction::Work` once and starts one worker; document and test the action count.
- There is no playback/audio, autosave, undo transaction, or command history here. Do not invent one. The only side effects
  are the connection test, store save, setup worker start, and demo CSV export.

No generic `Context` bridge: all widgets receive the existing `&mut Ui`; `Context::root_ui` would panic inside `App::ui`. No
shared helper is needed for only this crate.

### 4.4 Layout, row height, and narrow windows

0.37’s roomier defaults invalidate two hard-coded assumptions:

- `history.rs:38` uses `ROW_HEIGHT = 20.0` with `ScrollArea::show_rows`. The parameter is `row_height_sans_spacing`;
  `show_rows` itself adds `ui.spacing().item_spacing.y` (`crates/egui/src/containers/scroll_area.rs:1098-1106`). Pass only
  the content height excluding item spacing—normally `ui.text_style_height(&egui::TextStyle::Body)`—and do not add label
  spacing a second time. If a custom gap is needed, document and subtract it. Add first-row, middle and bottom scroll
  assertions, not just row containment. Test 1x/1.5x/2x and an actually larger text style; `pixels_per_point` does not by
  itself enlarge logical text.
- `progress.rs:33` reserves a fixed 90 points for Cancel. Use the actual button minimum width
  (`ui.spacing().interact_size.x`/button galley) plus spacing, or wrap Cancel below the bar at narrow widths. Add a 320×480
  test that Cancel stays inside the clip rect and is clickable.
- `status.rs:67` and `picker.rs:125` use unwrapped horizontal rows. Change action/header rows to `horizontal_wrapped` where
  commands would otherwise be pushed off-screen, preserving order and labels. `history.rs:157` already wraps; its footer row
  (`history.rs:229`) should wrap too.
- Keep panel ownership in the app. This crate creates no `SidePanel`, `TopBottomPanel`, or `CentralPanel`; examples use only
  `CentralPanel::default().show(ui, ...)`. Do not add a panel or docking abstraction.
- Do not change `LIST_HEIGHT = 380.0` in the picker or `LIST_HEIGHT = 320.0` in history without a measured reason; they are
  deliberate bounded lists.
- Preserve command reachability: a wrapped button stays visible, enabled/disabled as before, and keyboard-reachable.

### 4.5 Fonts and glyph coverage

Source strings are ASCII except escaped punctuation: `\u{2022}` bullet, `\u{2026}` ellipsis, `\u{2014}` em dash, `\u{00b7}`
middle dot. These are ordinary Latin-font glyphs, not color emoji, private-use icons, CJK, or combining marks. Still, 0.37’s
font stack changed and kittest panics on a missing glyph.

Rules:

- Production `egui` does not force bundled fonts. A consumer that supplies fonts keeps them; one that relies on eframe’s
  `default_fonts` gets them through eframe. That is not resolved-feature evidence; §4.1 requires per-root checks. Do not
  enable `system_fonts` by default; #8512 reports startup discovery cost.
- Tests use bundled default fonts and keep `MissingGlyphPolicy::Panic`. Add an explicit glyph test for the four code points;
  if one test deliberately needs tofu, scope `allow_missing_glyphs` to it and say why.
- No emoji or image glyph is rendered here, so there is no in-crate texture sampling to change. Do not add an image for
  decoration.
- If a future fixture uses Arabic/Thai/CJK/Hebrew/emoji, use the licensed pinned set in the egui plan §10.6 (craft-fonts
  `2481a8a...`) and record source, license and SHA256; do not copy proprietary/app font assets.

### 4.6 Screenshots, clipboard, IME, and viewports

- No screenshot request, `UserData` correlation or screenshot helper exists here; pixel tests use kittest snapshots. This
  library has no screenshot consumer and does not adopt the shared completion-slot helper.
- Shared capture contract: the suite rollout's `Shared components: justified boundaries` section is authoritative; no
  capture feature is added here. Defaults are 4 outstanding, 128 MiB/image, 256 MiB aggregate (stricter caps allowed);
  check `width*height*4` before issue, record issuance UI geometry/epoch, and cancel/retry on any resize/DPI/zoom/surface
  change (even back) before save. Callback carries pixels only; app owns strong slot+waker, callback is weak, unlocks
  before repaint and never wakes itself. The command forces a paint; queued-not-issued hidden capture/settling needs
  `RequestPaintWhileHidden` from logic. Adoption is atomic: the eleven `craft-ui` consumers take CL-1 and CL-2F in one
  dependency stage; Design/Grid use reviewed CL-2 provisional adapters before CL-2F freezes, and any incompatible
  `craft-ui` bump joins that stage. Fourteen capture consumers: eleven shared plus DeckCraft/FigCraft/ArtCraft Native
  local; launcher and this library have none.
- No clipboard, PRIMARY, file-drop, or pen code exists. Standard TextEdit receives host clipboard/IME events; do not
  intercept or translate them. Built-in TextEdit inherits the fork’s E21b producer. Native clipboard, PRIMARY, IME and focus
  remain host gates; this crate has no custom IME producer to adapt.
- No child viewport, docking, or hidden-window creation exists; #8748 does not apply. Do not call `Context::root_ui` from a
  widget or example; the existing `&mut Ui` is the root UI.

### 4.7 API compatibility summary

| Contract | Decision |
| --- | --- |
| Public widget functions | Keep signatures and return types. |
| `PanelEvent` variants | Keep `Saved(ConnectionId)` and `Closed`; add no exhaustive-match-breaking variant. |
| `ConnectionPanel::show` | Keep as compatibility helper; add `poll` and `ui`. Save/test now execute on the following poll, not inside the click frame. |
| `ConnectionPanel::form`, `ConnectionFormState`, `HistoryViewState` fields | Keep. |
| `StatusView` fields | Keep for this migration; stable role-ID work is a separate API decision. |
| Action enums/constants, persistence format | Keep. `Saved` still reports `ConnectionId`; shared-store schema unchanged. |
| Undo/commands | No undo/command system exists; no new one. |
| Feature flags | No new public flags. Snapshot/wgpu are dev-only. |
| Fonts | Production stops forcing bundled fonts; hosts own font policy. |
| Roles/names/IDs | Add accessible names and associations; add stable text/list salts; keep picker/status/combo salts semantically stable. |
| Screenshots | No screenshot API or helper; pixel goldens use kittest. |

## 5. Staged pull requests, dependency DAG, and rollback

All PRs are local drafts until a fresh independent review and explicit publication approval. Numbers are order labels, not
publication permission. The former separate stock-to-fork stage is absorbed into AI-2; there is no stock-0.37 production
stage.

### 5.1 PR series

| PR | Scope | Depends on | Rollback |
| --- | --- | --- | --- |
| AI-1 | Baseline harness on 0.36.2: deterministic fixtures, timing probe, geometry/accessibility assertions, and committed 0.36.2 pixel goldens using the qualified renderer below. No production edits. | plan review | delete new test/example files; no production change |
| AI-2 | **Atomic coherent graph and source compatibility:** manifest/lock/root `[patch]` for the approved storytold egui family + winit, explicit font/feature resolution per root, consumer-smoke workspace with one core identity and `cargo tree -d`, accessible names/`labelled_by`, stable ID salts, missing-glyph tests, kittest 0.37, candidate pixel run with reviewed diffs. Stock 0.37 is only a measurement/CI control, never a merged production stage. | AI-1; complete reviewed/published storytold P8 egui (P2b, P7 held-key pairing, P7b/E21b, P7c #8763/#8748 and justified perf fixes) plus completed winit (W15P, W27 lifecycle) | revert the whole stage to the 0.36.2 baseline; restore old lock/source |
| AI-3 | Lifecycle/root-Ui refactor: `ConnectionPanel` command queue, immediate `testing` admission, `poll(ctx)`/`ui` split, generation/cancellation, examples’ action queues, hidden-logic and repeated-pass tests. | AI-2 | revert one commit; restore old `show` path and examples |
| AI-4 | Responsive/visual hardening: row-height computation, wrapped action rows, progress width, expanded 320px and 1x/1.5x/2x pixel/geometry tests. | AI-2, AI-3 | revert UI-only commit; keep old constants/goldens |
| AI-5 | Consumer contract/docs: update `README.md`, `docs/LAYOUT.md`, `docs/PLATFORM-CONTRACT.md`, and a consumer recipe with exact pins, patch repetition, font/feature policy, the duplicate-core future-adoption constraint, and the fact that no app imports this crate. | AI-2, AI-3, AI-4 | revert docs; code unaffected |

Dependency DAG:

```text
AI-1 baseline 0.36.2 ───────┐
                             ├─► AI-2 ─► AI-3 ─► AI-4 ─► AI-5
approved fork + winit ───────┘
```

“Approved fork + winit” means that complete P8 egui revision plus the completed winit revision; stock 0.37 is only the measurement control.

App plans depend on AI-2/AI-3 only if they adopt `craft-ai-egui`; none currently does. Their own dependency reconciliation
(CADCraft/SoundCraft’s two egui versions, craft-ui pins) is not an AI-egui deliverable.

### 5.2 Rollback

The old coherent set is the entire baseline: `Cargo.toml` and `Cargo.lock` at `1a02afd6...`; `crates/ai-egui`
source/tests/examples at that commit; registry 0.36.2 egui family plus winit 0.30.13; no fork patch. Rollback is a revert of
the whole stage, not a lone version swap and not a stock-0.37 intermediate. Preserve the old `Cargo.lock` and source SHA in
evidence before the first dependency change. Do not change shared store/history/consent formats; no user-data migration is
needed.

### 5.3 Publication reachability and pin checks

- The repository is not a registry package (`publish = false`). Consumers use a pinned git revision.
- A consumer root that references the fork adds its own `[patch.crates-io]` entries for every egui-family package it actually
  uses plus winit; this repository’s patch is ignored by that consumer.
- The consumer-smoke workspace (separate `[workspace]`, path dependency on `crates/ai-egui` and `crates/ai-core`, its own
  exact `[patch.crates-io]`) runs the same tests with the fork and proves the recipe a real app will copy. Its `cargo tree` duplicate check must show one `craft-ai-core` and one egui family/one winit, and its type assertions must use that core identity. It
  must not pull `craft-libs/crates/ai-core`.
- Add a static check that rejects path patches in publication/manifests (except the intentional local path dependency in
  private development), branch/tag refs for egui/winit, a second registry egui family, and machine-local paths in
  docs/examples.
- Do not assume a version match is a source match. `egui_plot`, `egui_dock`, etc. are absent here; do not add or patch them
  without a consumer.

## 6. Regression and acceptance tests

### 6.1 Repository gates

These are the existing repository requirements (`.github/workflows/ci.yml`) and run under the managed `maintenance/task.py` run/status/finish workflow for implementation, not from this planning turn:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo check -p craft-ai-core --target wasm32-unknown-unknown --locked
cargo check -p craft-ai-features --target wasm32-unknown-unknown --locked
python3 -m unittest discover -s tools/mutation -p 'test_*.py'
cargo deny check advisories licenses sources
```

Focused gates:

```sh
cargo test -p craft-ai-egui --locked
cargo test -p craft-ai-egui --test panel --test widgets --test picker_view --test history_view --test consent_terms --test native_setup_view --test popup_view --locked
cargo run -p craft-ai-egui --example picker_demo
cargo run -p craft-ai-egui --example history_demo
```

No browser gate is claimed: `craft-ai-client`/`craft-ai-native` are native-only and `craft-ai-egui` is not a wasm target. The
`ai-core`/`ai-features` wasm checks stay as workspace gates; they are not browser UI tests.

### 6.2 User-observable assertions

1. **Accessibility and keyboard.** Every visible input has a name and `Harness::unnamed_widgets()` is empty after each
   settled harness. Name/Address/Key expose correct roles/labels; Tab follows visual order. Password is `PasswordInput` and
   key text never appears in labels/queries. Enter/Space activates the focused button; disabled controls are disabled in the
   accesskit tree; hidden controls (Key when `uses_key` is false) disappear.
2. **Stable identity and state.** Focusing Name, changing the test line/disclosure, and rerendering do not move focus/edit
   state. Typing in one field does not change another field’s text/selection. Picker scroll and row selection survive an
   unrelated update; changing a combo selection does not move focus.
3. **Connection test lifecycle.** Test starts exactly one localhost request; a forced second UI pass cannot start another and
   sees Test disabled immediately. A late answer for a changed endpoint/generation is dropped; the form stays untested and no
   old models are saved. `poll` without drawing eventually reports the result. Hidden poll requests repaint only while
   testing and stops after completion. Editing/cancel clears the old result and a canceled generation never applies a late
   answer.
4. **Save/close/export side effects.** Save produces one `PanelEvent::Saved` and one store write, including under a forced
   second pass. Invalid save keeps the form open with the same error and writes nothing. Cancel produces `Closed` and writes
   nothing. A save completing in the same frame as Cancel queues `Saved` then `Closed`; both are observed delivered. Export
   produces one CSV file, not one per pass; Clear still needs the explicit second confirmation. Native setup
   `Work` is returned once per admitted request; `busy` blocks a duplicate and a late completion is refused (existing tests).
5. **Geometry and hit testing.** Picker: clicking the center of the first/last visible row chooses the intended model; a
   disabled group produces no action; Set up/Fix works. History: a 3,000-row fixture draws only visible rows; first/last
   visible lines match the filter; export/clear stay inside the clip at 320×480. Progress: Cancel is fully visible and
   clickable at 320×480; percentage/cost do not overlap. Status/picker/consent rows wrap rather than hiding commands at
   320px. At 1.0/1.5/2.0 pixels-per-point, rows do not overlap and controls stay inside clip rects.
6. **Themes, fonts, text.** Dark/light themes use theme colors with no hard-coded foreground. The fixture covers U+2022,
   U+2026, U+2014, U+00B7; missing glyphs panic by default. No tofu in the fixed fixture; no claim about arbitrary user text
   or color emoji. RTL/CJK/combining tests are host-enabled only; the English strings claim no RTL support and add no
   fork-only RTL API.
7. **Persistence/workflow.** Save a synthetic connection, close, and reopen a fresh `ConnectionPanel` on the same temporary
   store: connection and models are present; the secret key is only in the in-memory secret store. History filter/offset/tab
   and CSV columns are unchanged; old store files still load.

### 6.3 Pixel comparisons (mandatory)

A deterministic visual suite is a required AI-1/AI-2/AI-4 deliverable, not an optional question. Geometry and semantic tests
do not catch changed styling, clipping, contrast, theme colors, or a visually broken wrapped control.

- Add `egui_kittest` dev features `snapshot` + `wgpu`. Use `HarnessBuilder::wgpu()` or
  `HarnessBuilder::wgpu_setup(WgpuSetup)` (wrapping `WgpuTestRenderer::from_setup`, `builder.rs:251-252`), not the default
  lazy renderer. Baseline 0.36.2 and candidate 0.37 use different kittest/egui-wgpu/wgpu versions: record those versions,
  adapter driver and snapshot settings; version-induced diffs are attribution limits, or add a stock-0.37 visual control.
- **Qualify the adapter.** `WgpuTestRenderer` prefers a CPU adapter but can fall back to a physical GPU (candidate
  `crates/egui_kittest/src/wgpu.rs:30-55`). Record backend/device type/name/driver for every run and enforce a CPU adapter
  (for example lavapipe/software) for committed goldens. Physical-GPU images are a separate diagnostic row and must not
  update goldens. If the CPU adapter cannot be qualified, record that as an explicit blocker and require consumer-app
  screenshot evidence; do not leave the visual gate optional.
- Fix the harness: 800×600 and 320×480, 1x/1.5x/2x, dark/light, explicit `Os`, bundled test fonts, animations disabled by
  kittest, stable initial state. Record the exact font files/hashes and license manifest.
- One image per widget family: consent, status, progress, picker, popup/install, history jobs, history empty, connection
  form. Use kittest’s per-pixel threshold and a very small `max_failed_pixels`; do not raise tolerance to hide a real shift.
- Baseline is captured at 0.36.2 before production edits. Candidate differences from 0.37 style changes are reviewed
  individually and recorded as intended. Never bulk-refresh goldens to make a failing suite green.
- **Actual implementation note:** this checkout did not capture the 0.36.2 baseline images. The new
  `crates/ai-egui/tests/visual_snapshots.rs` suite renders the 0.37 candidate only; its five
  committed PNGs are candidate baselines, not before/after migration attribution. The final run
  kept all five hashes unchanged, and a pure-background RGB(27,27,27) to RGB(30,30,30) negative
  control failed as expected with 75,486 changed pixels.
- Include a negative control: deliberately shift a row/button or remove an accessible name in a temporary mutation and
  confirm the visual or semantic test fails; restore source afterward.
- Keep pixel tests separate from native screenshots. An offscreen kittest image does not prove native focus, IME, clipboard,
  or window behavior.

### 6.4 Native/platform evidence (explicit gates)

Native acceptance is blocking for this library’s standard widgets; headless kittest does not replace it. Use fixtures
appropriate to a library with no document model: the two examples plus a synthetic connection save/reopen through the
temporary shared store. Do not invent document, autosave, undo, or screenshot semantics.

| Row | Required evidence |
| --- | --- |
| Linux headless | Qualified kittest CPU tests. The host’s `/dev/dri/renderD128`/i915 is availability evidence only, not a qualified backend/compositor result. |
| Linux X11 | Real session/window manager; recorded server/compositor identity; actual focus, keyboard, clipboard and TextEdit/IME behavior; example/panel screenshots. |
| Linux Wayland | Real compositor and version (wlroots/KWin/Mutter); focus, keyboard, clipboard/PRIMARY where exercised, and E21b/P7b IME/outgoing surrounding text; screenshots and protocol trace where applicable. |
| Windows | Uniquely owned managed clone with a visible desktop; examples plus a real connection panel: type into fields, run a localhost/scripted test, save a synthetic connection, reopen it, capture edited/saved/reopened screenshots; verify the active console, not session 0. |
| macOS | Explicit unresolved gate: real IME/Metal/termination evidence or a recorded blocker; cross-compilation/kittest is not acceptance. |
| Browser | Not applicable: the client stack is native-only; do not claim a browser UI pass. |

### 6.5 Deliberate-bug checks

Rerun the existing groups after the migration:

```sh
python3 tools/mutation/mutation_check.py ce
python3 tools/mutation/mutation_check.py nw
python3 tools/mutation/mutation_check.py v2
python3 tools/mutation/mutation_check.py v3
```

Add a focused `panel`/`eg` group:

| Mutant | Test that must fail |
| --- | --- |
| Remove `labelled_by`/`accessible_name` from Name | accessibility test / `unnamed_widgets()` |
| Give two TextEdits the same ID salt | stable-ID/focus test |
| Execute `manager.save` directly in `ui` and again in `poll` | exactly-one-save test |
| Clone instead of take `pending` | exactly-one-test/save test |
| Start `TestRunner` in `ui` as well as `poll` | server request-count test |
| Leave `testing` unset when enqueueing StartTest | immediate-disabled/second-pass test |
| Ignore endpoint generation/cancellation | late-answer generation test |
| Poll the runner only inside `ui` | hidden/logic-only poll test |
| Remove the captured-address check | late-answer provenance test |
| Revert `horizontal_wrapped` in status/picker | 320px command-reachability test |
| Use a hard-coded 20px row **including** item spacing | row-overlap/clip test |
| Remove the glyph test’s font setup | missing-glyph panic test |

A mutant that does not compile is not “caught.” Fix the mutant or the test; do not ignore a survivor. Run the group
exclusively, restore source, and rerun the focused tests clean.

Final qualification run: the `eg` group caught both compiled lifecycle mutants
(`save-frame-dedup-removed` and `answer-generation-ignored`), restored `panel.rs`, and the clean
package tests passed again. The other mutation groups listed above were not rerun in that job.

## 7. Performance protocol

No performance result is claimed by this plan. The shared suite minimums apply and may be tightened, never loosened.

### 7.1 Workloads, measurement and gates

Record source/lock SHA and package IDs, per-root features, toolchain/profile, OS/kernel, CPU/RAM, GPU/driver and
renderer/backend, session/compositor, DPI, theme, viewport, font files/hashes, fixture identity, warmups, sample counts and
host load (no concurrent build/VM/agent).

Fixtures: (1) history 3,000 deterministic jobs, jobs/day/month grouping, empty/filtered, scroll top/middle/bottom, long
labels, 1280×720 and 320×480, 1x/1.5x/2x, dark/light; (2) picker 20×20 models, mixed ready/needs-key/off, presets, 560×640
and 320×480; (3) connection form blank/edit, 2 KB address, long pass/fail line, key shown/hidden, 100-character typing and
localhost test; (4) status/consent/progress/popup small widgets with 20 choices/items; (5) 20 fresh example processes with
first `Context` pass and font/window setup separate from compile time. Use synthetic data only; no
provider/keychain/download. Script fixtures use the licensed pinned font set/hashes (§4.5, egui plan §10.6).

Measure wall time for `Context::run_ui` + `tessellate`, tessellation shape/vertex/index counts, galleys, allocations/peak RSS
(and guest working set), frame distributions (≥1,000 frames, p50/p95/p99), idle repaint count/CPU over 10 s,
Test/Save/Export/setup `begin` latency, startup/font-provider timing and raw samples.

Protocol: ≥10 warmups and 30 timed iterations; 20 startup processes; 1,000 frames; alternate baseline/candidate order with
separate allocations/processes and identical compiler/profile/features/fonts/fixture/backend/viewport; software and real GPU
are separate classes; no noisy workloads; retain logs/commands. Every first baseline report and investigated regression
also runs the equivalent stock-0.37 control (a pinned minimal adapter where fork-only code does not compile) with all three
source/lock identities; unavailable equivalence is an attribution limitation.

Predefined gates: investigate repeatable >5% median or >10% p95 layout/tessellation/interactive regression, >5% peak RSS,
any new idle busy-loop/repeating repaint/absolute-budget failure; for sub-millisecond cases require >0.05 ms or corroborating
instruction/allocation counts. Rerun noise; never widen tolerance after a failure. Intentional tradeoffs need documented
review.

### 7.2 Targeted upstream probes

These are external reports, not reproduced here.

- **#8731 text tessellation:** compare unchanged 0.36.2, stock 0.37.0 and the selected fork on history/picker fixtures with
  `perf stat -e instructions:u` (Linux) and wall time. If the reported plain-text tessellation regression reproduces, do not
  accept it silently and do not add a clamp-only texture change. Evaluate a per-row fast path only with color-glyph rows
  covered; verify identical vertices/indices and pixel output for mixed/color rows. Textures/wrap/filter semantics stay
  unchanged.
- **#8736 DX12 pipeline creation:** not measurable in this crate’s headless tests. When an adopting app uses eframe/wgpu on
  Windows DX12, time `Renderer::new` and first frame separately from build time; compare stock/fork and a shader fix that
  preserves Repeat/MirroredRepeat, not clamp-only.
- **#8512 system-font startup:** measure separately only if a host enables `system_fonts`; preserve glyph coverage and
  compare configured providers; do not enable it in `ai-egui` defaults.
- **#8763/PR #8764:** no selection-sensitive action exists here. If an app adds one, test click-away before/after the fix; do
  not patch egui in this repository; fork P7c owns the selected #8763 fix, PR #8764 is open/unmerged.
- **#8748:** no viewport code here. Consumer apps test deferred child windows; stock 0.37.0 lacks the upstream
  closed/unmerged PR, and fork P7c owns the selected fix.

## 8. Risks, unresolved questions, and acceptance blockers

### 8.1 Blocking acceptance criteria

- `cargo fmt`, strict clippy, workspace tests, wasm checks for `ai-core`/`ai-features`, and `cargo-deny` pass on the exact
  candidate revision under managed jobs.
- Every kittest input has an accessible name; no test disables the accessibility check.
- Test/save/export/setup actions are exactly-once under repeated UI passes; queued StartTest disables admission immediately,
  a hidden `poll` drives a running test without a paint, endpoint generation/cancellation is covered, and concurrent
  `Saved`+`Closed` are both delivered.
- Existing provenance, consent, setup-ticket, and shared-store semantics remain covered.
- Pixel suite passes with the qualified kittest wgpu renderer/CPU adapter and recorded kittest/wgpu/driver identities;
  every changed golden is individually reviewed; no bulk refresh.
- Performance baseline, candidate and every investigated regression record raw samples, environment details and the
  equivalent stock-0.37 control per §7; no regression is hidden by tolerance changes.
- The production graph pins the approved storytold egui family and winit by immutable full SHAs, repeats patches at the
  consumer-smoke root, and produces one egui family/one winit in `cargo tree`. The smoke root proves one intended
  `craft-ai-core` identity.
- Feature resolution is checked per root; the production graph does not silently enable `default_fonts`;
  tests/examples/consumer smoke each record their policy.
- **Native platform gate:** Linux X11 and Wayland with recorded compositor identity, actual focus/clipboard/TextEdit/IME
  behavior, and screenshots; Windows in a uniquely owned clone with a visible edited/saved/reopened connection workflow and
  screenshots. macOS IME/Metal/termination is an explicit unresolved blocker, not a silent pass. Record exact commands,
  fixtures, screenshot paths and actual selected test counts. If the library intentionally defers native evidence, state that
  migration is not acceptance-complete for native use.
- A fresh independent read-only review of the final test packet is complete. Native/platform and publication review remain
  open.
- Explicit human publication approval is obtained for each push/new PR.

### 8.2 Unresolved questions

1. **No current app consumer.** Checkout evidence says library-only. The migration defines a consumer recipe and smoke
   workspace but invents no downstream acceptance; adoption gets its own PR, pin, font policy and tests.
2. **Stable status ID.** `StatusView` has no role ID; its label is an ID salt. A later API may add a stable role id
   (breaking) or keep the documented English-label requirement. This migration does not change it.
3. **Font ownership.** Each adopting app must supply fonts or enable eframe `default_fonts`; a custom host font lacking
   bullet/ellipsis/em dash/middle dot needs its own glyph test.
4. **#8731.** If upstream/fork text tessellation is materially slower, a reviewed fork fix may be required before rollout; a
   documented exception for a text-light widget set needs explicit review. Measure first.
5. **Fork timing/SHAs.** Approved integrated storytold egui/winit SHAs do not exist yet. AI-2 is blocked until publication;
   SHAs are recorded, never invented. aiongg candidates remain research inputs.
6. **Duplicate core.** One authoritative `craft-ai-core` is future `CORE-SYNC` work, not part of this migration. Until then
   the smoke root stays on one identity and apps must not combine both core packages.
7. **Native evidence.** macOS IME/Metal, physical Windows GPU, Linux compositor/PRIMARY and screen-reader behavior remain
   environment gates; no planning shell has a display session.

### 8.3 Known limitations of this plan

- The original source audit ran no compile or test; API impact there was inferred from 0.37.0 source/changelog. The final
  headless follow-up qualification covers 108 tests, five candidate snapshots, the `eg` lifecycle mutants and the locked
  consumer smoke; it does not cover native workflows, physical GPUs, performance or the other mutation groups.
- The consumer table reflects the exact planning checkouts; re-inventory before final integration.
- The fork/winit candidates are not accepted storytold revisions; E1–E21, W15P, E4/P2b and E21b/P7b are not revalidated here.
  Upstream #8731, #8736, #8512, #8763, #8764 and #8748 are external reports.
- No native screenshot, IME, clipboard, PRIMARY, accessibility-tree, GPU or performance result is claimed. `docs/LAYOUT.md`
  no longer claims PhotoCraft or launcher path dependencies; the planning checkouts did not reproduce one.

## 9. Publication and review

This document is a local planning draft. Do not push it, open a PR, comment on upstream issues, merge, release, or send it to
another chat. Before any publication: show the exact branch/commit range, files, and validation commands/results; obtain
explicit human approval for that publication; keep the plan and later implementation reviews in the dedicated evidence
directory; preserve the old coherent dependency set for rollback; add no agent/model/tool attribution; and keep contributor
metadata username-only (`@skylartaylor`) without altering other contributors.


## Local implementation update — 2026-10-23

The user clarified that reviewed plans remain local and implementation proceeds before publication. The earlier requirement to publish/land this plan first is superseded; pushes and PR publication still require explicit approval.

Canonical `storytold/craft-ai-integrations` `main` now resolves to `1759a52660b138f8cae7ba848774a49b7572b0ab`. The owned checkout was fast-forwarded to this revision. Earlier baseline SHAs and line anchors above are retained as historical review evidence; use this new revision and its exact lockfile for current-main integration and paired measurements. The original refs remain reachable.

craft-ai-native is now a normal ai-egui dependency. Preserve the one-button setup UI, UseSuggestedFolder action, accessible blocker labels, route advice and new first-run/one-button tests.

The complete static delta review and exact old/new object identities are retained in the task evidence: `canonical-diffs-refresh-20261023.json` and `overnight-consumer-review-20261023.md`. This amendment claims no new build, native GUI or performance acceptance.

### Current dependency state: reviewed storytold pins, not acceptance

The checkout now resolves the reviewed storytold egui family at
`6d518fc799681571963842f95b55a6b68431b15a` and winit at
`a517b3846ee8457d84ee4975110bfda6797c44dc`. The root `[patch.crates-io]` supplies the family and
native event bridge, and `consumer-smoke/` repeats that recipe because patches do not propagate to
a consumer root. The root lockfile SHA-256 is recorded above.

This dependency state satisfies the pin half of AI-2. The follow-up source and headless tests are
now qualified as recorded above: 108 tests, five candidate snapshots, the `eg` lifecycle mutant group
and the locked consumer smoke. Native/platform and performance evidence remain absent, the other
mutation groups were not rerun, and the repository has no published app adoption. No
acceptance-complete claim is made; publication still needs explicit human approval.

## Human-review readiness summary — 2026-10-24

The documentation now records only the qualification actually run for this follow-up: 108 passing
tests, strict all-target clippy and fmt, five hash-stable candidate snapshots, both compiled
connection-panel lifecycle mutants caught, the negative pixel control caught at 75,486 changed
pixels, restored source and goldens, a passing locked consumer smoke with one core identity, and a
passing independent test-packet review.

Genuinely open AI gates: native Linux X11/Wayland and Windows focus/keyboard/clipboard/IME/rendering
screenshots; macOS IME/Metal/termination; physical-GPU qualification; the performance protocol; any
app adoption; the migration's other mutation groups; CORE-SYNC; and explicit human publication
approval. This checkout remains a review draft, not a merge or release.
