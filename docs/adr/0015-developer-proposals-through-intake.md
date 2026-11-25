# ADR 0015: Developer suggestions go through the intake path

- **Status:** accepted 23/10/2026
- **Source:** [#103](https://github.com/storytold/craft-ai-integrations/pull/103), [PROPOSE-AN-INTEGRATION.md](../PROPOSE-AN-INTEGRATION.md)

## Context

There was no set path for a developer, or an agent working for one, to suggest a model, connection, engine or workflow, and nothing kept "suggested", "implemented" and "reviewed" apart.

## Decision

Suggestions start at `docs/PROPOSE-AN-INTEGRATION.md` and `tools/intake/propose.py`, which scaffolds and checks a proposal without network calls and refuses secret-shaped fields. A suggestion is metadata only: not a vetted model, a working connection, a user binding or permission for a paid call.

## Consequences

Credentials and user bindings stay out of proposals and version control. A proposal becomes a built-in only through review.
