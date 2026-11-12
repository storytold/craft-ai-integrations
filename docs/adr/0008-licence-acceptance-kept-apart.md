# ADR 0008: Accepted model licences are kept apart from installed files

- **Status:** accepted 17/10/2026
- **Source:** [#32](https://github.com/storytold/craft-ai-integrations/pull/32)

## Context

An acceptance stored on an installed file's entry disappeared when the entry was removed, and a damaged record read as "not accepted" only after a full download.

## Decision

Accepted licences are their own list in the registry: kind of source, pinned version, licence name and link, and when. `install` records the yes once the checks pass, so a failed download does not lose it. A damaged record is reported as an error before anything is fetched.

## Consequences

A person accepts a model's terms once per pinned version, and is told about a damaged record before a download starts.
