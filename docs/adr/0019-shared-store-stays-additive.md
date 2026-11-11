# ADR 0019: The shared store stays additive, and a schema bump is the last resort

- **Status:** accepted 05/11/2026
- **Source:** [STORE-SCHEMA.md](../STORE-SCHEMA.md), [#156](https://github.com/storytold/craft-ai-integrations/pull/156)

## Context

Every app pins its own revision but shares one store file, so an older build often reads and rewrites a file saved by a newer build.

## Decision

`SharedStore` keeps fields it does not know at every depth, matches list items by `id`, `role` or `capability`, and refuses a file with a higher `schema_version`. Changes to the file are made additively wherever possible.

## Consequences

A schema bump shuts every older app out of the person's connections until it updates, so it is used only when an additive change cannot work. STORE-SCHEMA.md says which changes need one.
