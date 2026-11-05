# Keeping the contract aligned with craft-libs

`craft-ai-core` is accepted in `storytold/craft-libs` (PR #69). Its address-aware model-test API
was updated by #166. This workspace still uses a local copy, not a git dependency on that crate.
Do not interpret acceptance of the initial contract as proof that both copies are interchangeable.

## Upstream fixes carried here

The contract changes through upstream commit `559992953fb4c36125614f152d24b164d0c7aadd` are
represented by these regression guards:

- A model must accept every input a tool sends **and require no input the tool cannot supply**.
  `fits`, `eligible`, binding resolution, app status and planning all agree. A refusal occurs
  before a key is read or anyone is asked to consent.
- A test result is recorded with `ConnectionDraft::set_models(address, models) -> bool`. Pass
  the address captured when the test started, never the address currently displayed in a form.
  A late result for a different address is refused without replacing the form's models.
- Loaded, preset and tested models keep their address provenance through repeated edits.
  Returning to an old address does not turn another address's models into its own.
- Surrounding whitespace in a saved address does not make its existing models disappear.

The shared connection panel passes its captured address and only reports a passing result when
`set_models` takes the models. Third-party callers must update the old one-argument call too.
Assigning `draft.models` directly is not a replacement for recording a probe result.

## Why the dependency has not switched yet

Known additions used by integrations but absent from the inspected upstream contract:

| Addition | Why it cannot simply be dropped |
|---|---|
| `ConsentTerm` and the consent-ledger convenience methods in `consent_term.rs` | Consumers use shared consent durations and temporary agreements. |
| `model_folders` and `StoreFile::model_folders`, its default and validation errors | The client stores the person's shared model-folder map in this field. Removing it would stop that API compiling or lose the map on a rewrite. |

These are pure contract additions and need their own upstream review. No change here authorises
opening a PR or publishing them in craft-libs. The local `draft/presets.rs` split is only file
organisation and does not require a different public API.

## Switching to one authoritative crate

1. Recheck the live upstream crate before starting; it may already contain some additions.
2. Review the consent-term and model-folder additions upstream as separate focused changes,
   preserving old-store defaults and validation. Do not silently change the storage format.
3. Once the required APIs exist, replace the workspace's local `craft-ai-core` dependency with
   a pinned, immutable craft-libs git revision and remove the duplicate workspace member.
4. Run the core regression guards against that dependency, the client and widget suites,
   old-store/folder round trips, the adoption examples and the applicable wasm checks.
5. Pin app dependencies separately in each app's own PR. Keep keys in the secret stores.

The exact upstream placement and review outcome are not assumed. Until the prerequisites land,
retain the local copy and keep its safety behaviour aligned rather than replacing it blindly.

## Verification entry points

- `crates/ai-core/tests/draft_provenance.rs`: endpoint edits and late test results.
- `crates/ai-core/tests/required_inputs.rs`: model compatibility and binding resolution.
- `crates/ai-client/tests/required_inputs.rs`: app-facing consistency and no early key access.
- `python tools/mutation/mutation_check.py ca`: deliberate bugs for those safety boundaries.

Run the mutation group exclusively, then rerun clean tests after source restoration. These checks
use temporary stores, not a real keychain, provider, download host or Linux sandbox.
