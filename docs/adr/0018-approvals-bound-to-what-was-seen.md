# ADR 0018: Approvals and plans are bound to what the person saw and to one app

- **Status:** accepted 01/11/2026
- **Source:** [#117](https://github.com/storytold/craft-ai-integrations/pull/117), [#118](https://github.com/storytold/craft-ai-integrations/pull/118)

## Context

A yes given in a consent prompt could be applied to a different destination if settings changed while it was open. A second app's client could run the first app's plan under the first app's consent.

## Decision

Sample approval carries the disclosure the person reviewed, so a changed destination or payload asks again. `AiClient::start` refuses a plan from another app before checking consent, reading a key or building a provider. Clients with the same app id can still share plans.

## Consequences

`ToolDesk::start_test` and `start_sample` take the reviewed `Disclosure`, not a boolean. App ids are caller-supplied, so this prevents accidental reuse, not a malicious host process.
