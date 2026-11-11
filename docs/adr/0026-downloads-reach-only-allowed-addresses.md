# ADR 0026: Downloads connect only to addresses a link may reach

- **Status:** accepted 11/11/2026
- **Source:** [#152](https://github.com/storytold/craft-ai-integrations/pull/152)

## Context

A link's DNS answers were checked, then the hostname was resolved again for the connection, so a name whose answers change could reach an internal address. The model downloader checked hostname text only.

## Decision

`craft_ai_media::links::GuardedResolver` wraps the default resolver and fails the connection unless every resolved address is allowed: not private, loopback, link-local or another internal range, with loopback only when allowed. Media copies and model downloads both connect through it, on every redirect hop. TLS still verifies against the original hostname.

## Consequences

A new downloader must build its agent with the guarded resolver. The early plain-language refusal stays as a first check.
