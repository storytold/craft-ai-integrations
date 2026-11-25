# ADR 0002: A chosen model is never replaced silently

- **Status:** accepted 13/10/2026
- **Source:** [PLATFORM-CONTRACT.md](../PLATFORM-CONTRACT.md), [#38](https://github.com/storytold/craft-ai-integrations/pull/38)

## Context

A registered tool is a declaration, not installed code. A compatible binding is not proof that a provider can run the request in a given app.

## Decision

When the selected adapter is missing in an app, that app refuses the job at `plan`, before the consent prompt and before reading a key, and says so. It never substitutes another model or engine. A tool with no model chosen is reported as needing a model, not as broken.

## Consequences

A person always gets the model they chose, or a plain refusal. Apps must handle the refusal rather than fall back.
