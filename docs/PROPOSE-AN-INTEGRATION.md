# Propose an AI model, engine, connection or workflow

This is the **developer path for suggestions that ArtCraft may vet and ship**. It does not change
any person's custom models, saved connection, key, binding or consent. An issue is not a catalogue
entry; a catalogue entry is not a working adapter; a passing scripted test is not a live app run.

## Start from any Craft app

Tell your agent: "Propose [service/model/workflow] for [apps]. Check what already exists and prepare
it for ArtCraft review." The agent reads this guide, the app's `AGENTS.md`, and the relevant
[adoption guides](adopting/README.md). It checks the live shared catalogue, adapters, app code,
open issues and provider documentation before naming the missing piece. Do not treat a model
mentioned on an ArtCraft website as a working route in the Craft apps.

From a checkout of `storytold/craft-ai-integrations`:

```sh
python3 tools/intake/propose.py new proposal.json --title "Example 3D workflow" --kind workflow --app cadcraft
# Fill problem and sources with verifiable documentation. Leave unknown facts null/unknown.
python3 tools/intake/propose.py check proposal.json
python3 tools/intake/propose.py issue proposal.json --out issue.md
```

The file created by `new` is the **template**. Its blank fields are intentional. All four kinds
(`model`, `connection`, `engine`, `workflow`) use it. The CLI neither fetches URLs nor posts to
GitHub. Review `issue.md`, then a contributor who has permission to publish can submit it to
`storytold/craft-ai-integrations` using the feature-request form or `gh issue create`.
No automated agent should publish, call a paid service, download weights, bind a user's tool or
install code without its developer's explicit approval for that action.

For the CADCraft example, check whether a Tripo model or connection is already available in the
shared layer before creating a duplicate. Identify the exact requested input (prompt, source
image, or both), the actual service endpoint and version from its publisher, and the output that
CADCraft can import. **Do not infer that mesh output works because `mesh.generate` is a known
capability ID.** A proposed GLB result has to survive safe download, media-type checking, app
import, undo, save and reopen. The current media copier recognises image, audio and video
formats, not mesh files. This is a real implementation gap, not a new catalogue row.

## From suggestion to live build

1. **Suggestion:** `check` validates the shape, mandatory problem and documentation URLs, and
   flags some secret-shaped fields and values. It does not verify the publisher, licence, price,
   model capabilities or safety of an endpoint. A human reviews the issue.
2. **Implementation:** choose the route in `integration.route` from `existing`, `declarative`,
   `provider`, `native-engine` or `comfyui`. Run `python3 tools/intake/propose.py check
   proposal.json --stage implementation` to catch missing metadata and known mismatches. This
   is still a static check, **not** an ArtCraft vetting badge. The declarative route currently
   accepts only a prompt, and mesh results need a new durable-result path. Refer to
   [Bring a service](adopting/bring-a-service.md), [Add an engine](ADD-AN-ENGINE.md),
   [the app guide](adopting/use-in-your-app.md), and [the checklist](adopting/checklist.md).
3. **Review:** a focused shared-layer PR contains the adapter/recipe and scripted conformance
   tests; an app PR contains its tool UI, document import and result persistence. If a contract
   change is needed, review that in `craft-libs` first. Test errors, cancellation, consent,
   destinations, licence disclosure and secrets. Keep user-defined entries separate from built-in
   suggestions and never replace a person's binding. State what was actually run, including any
   paid or live service calls.
4. **Vetting:** ArtCraft decides whether the exact version and workflow belong in the curated
   catalogue and release. Only after an approved real-result test in the app may anyone say the
   workflow works there. Review and merge are human decisions, not a validator side effect.

**Connections are definitions, not accounts.** A contribution can describe where and how to
connect, but no contributor submits a key or someone else's stored connection. The user enters
credentials in the app or launcher and separately agrees to send data. New local engines require
a tested runner and an app build that includes it; adding its name to a recipe is insufficient.

The intake checker is deliberately small and has no dependency on Rust or an external schema
library. Its guardrails are not a security scan: keep secrets out of proposal text and issue
bodies even when it says "passes". Maintain its known-route rules as the runtime gains support.
