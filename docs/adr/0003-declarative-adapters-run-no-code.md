# ADR 0003: Declarative adapters run no code

- **Status:** accepted 13/10/2026
- **Source:** [design/declarative-adapter.md](../design/declarative-adapter.md), [#17](https://github.com/storytold/craft-ai-integrations/pull/17)

## Context

Some providers are not OpenAI-shaped. Writing a Rust adapter for each one is slow, and running code supplied with a connection would be unsafe.

## Decision

A provider can be described by a data file that `craft-ai-declarative` runs through the existing `Provider` trait. The file holds no scripts and no expressions. It names the calls, the authentication and where results are found.

## Consequences

Apps, consent, history and the conformance suite see nothing new. A provider that needs logic beyond the file's vocabulary still needs a Rust adapter.
