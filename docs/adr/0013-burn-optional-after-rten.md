# ADR 0013: Burn is an optional engine after rten

- **Status:** accepted 22/10/2026
- **Source:** [#91](https://github.com/storytold/craft-ai-integrations/pull/91)

## Context

With the Burn engine built, a person with a graphics card had no way to choose it, but rten already ran SAM 2.1 everywhere.

## Decision

`craft-ai-client` gets an optional `burn` feature that registers the engine. SAM 2.1 lists a `burn` route right after `rten`, with the same two published files, sizes and hashes. rten stays first and stays the default.

## Consequences

An app opts in to Burn at build time. Offering the route downloads and binds nothing.
