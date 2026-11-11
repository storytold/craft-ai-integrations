# Architecture decisions

One file per decision that shapes this layer, numbered in the order the decisions were made.

## Adding one

1. Copy the newest file, take the next number and give it a short slug.
2. Fill in Status, Source, Context, Decision and Consequences. Keep it short and link to the doc
   or pull request that holds the detail.
3. Add it to the table below, in the same pull request as the change it explains.

Record only decisions that were made. A proposal or an open question belongs in its issue. A
decision that replaces an earlier one gets a new file, and the old one's Status says
`superseded by NNNN`.

| ADR | Decision | Accepted |
|---|---|---|
| [0001](0001-shared-crates-own-the-connection.md) | The shared crates own the connection, each app owns its interface | 12/10/2026 |
| [0002](0002-never-replace-a-chosen-model.md) | A chosen model is never replaced silently | 13/10/2026 |
| [0003](0003-declarative-adapters-run-no-code.md) | Declarative adapters run no code | 13/10/2026 |
| [0004](0004-one-tls-setting.md) | One TLS setting for every network call | 14/10/2026 |
| [0005](0005-consent-is-scoped-and-can-lapse.md) | Consent is scoped to one use and can lapse | 14/10/2026 |
| [0006](0006-local-engines-share-one-runner-contract.md) | Local engines share one runner contract | 17/10/2026 |
| [0007](0007-model-folders-kept-as-a-map.md) | A person's model folders are kept as a map, never reorganised | 17/10/2026 |
| [0008](0008-licence-acceptance-kept-apart.md) | Accepted model licences are kept apart from installed files | 17/10/2026 |
| [0009](0009-keys-travel-in-headers.md) | Keys travel in headers, never in a URL | 19/10/2026 |
| [0010](0010-keep-local-craft-ai-core.md) | Keep a local `craft-ai-core` until craft-libs has what it lacks | 20/10/2026 |
| [0011](0011-sam21-first-for-select-subject.md) | SAM 2.1 is offered first for Select subject | 21/10/2026 |
| [0012](0012-time-goes-through-chrono.md) | Time goes through `chrono` | 21/10/2026 |
| [0013](0013-burn-optional-after-rten.md) | Burn is an optional engine after rten | 22/10/2026 |
| [0014](0014-an-offer-never-chooses.md) | An offer never chooses a model | 23/10/2026 |
| [0015](0015-developer-proposals-through-intake.md) | Developer suggestions go through the intake path | 23/10/2026 |
| [0016](0016-widgets-on-the-suite-egui-fork.md) | Shared widgets use the suite's egui 0.37 fork | 25/10/2026 |
| [0017](0017-remove-background-built-in.md) | Remove background runs built in, with InSPyReNet on Burn | 30/10/2026 |
| [0018](0018-approvals-bound-to-what-was-seen.md) | Approvals and plans are bound to what the person saw and to one app | 01/11/2026 |
| [0019](0019-shared-store-stays-additive.md) | The shared store stays additive, and a schema bump is the last resort | 05/11/2026 |
| [0020](0020-report-what-this-build-can-run.md) | Report what this build can run apart from what is saved | 05/11/2026 |
| [0021](0021-cost-estimate-before-consent.md) | A cost estimate is shown before consent, and never guessed | 09/11/2026 |
| [0022](0022-clippy-enforces-coding-standards.md) | Clippy enforces the coding standards | 09/11/2026 |
| [0023](0023-retry-only-where-safe.md) | Retry only where a repeat cannot charge twice | 10/11/2026 |
| [0024](0024-encrypted-file-secret-store.md) | Encrypted file as the last secret store | 10/11/2026 |
| [0025](0025-resume-against-named-version.md) | Resume a cut download only against a named file version | 11/11/2026 |
| [0026](0026-downloads-reach-only-allowed-addresses.md) | Downloads connect only to addresses a link may reach | 11/11/2026 |
