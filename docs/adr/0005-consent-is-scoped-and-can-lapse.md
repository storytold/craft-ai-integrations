# ADR 0005: Consent is scoped to one use and can lapse

- **Status:** accepted 14/10/2026
- **Source:** [#21](https://github.com/storytold/craft-ai-integrations/pull/21), [#25](https://github.com/storytold/craft-ai-integrations/pull/25)

## Context

A saved connection is not permission to send a person's data anywhere. An agreement given for one tool must not cover another, and people need to agree for a limited time.

## Decision

An agreement covers one app, connection, destination and capability, for the kinds of data it named, and may carry an expiry. `start` checks again, so a lapsed agreement stops a job before the key is read. Records saved before the capability field existed cover nothing. `ConsentTerm` names the four standard durations: this time, an hour, a day, until revoked. "This time" is never saved.

## Consequences

A person may be asked again after an upgrade or when an agreement lapses. Each host chooses its own wording for the terms, but uses the shared ids.
