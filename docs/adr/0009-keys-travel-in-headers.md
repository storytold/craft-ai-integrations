# ADR 0009: Keys travel in headers, never in a URL

- **Status:** accepted 19/10/2026
- **Source:** [#52](https://github.com/storytold/craft-ai-integrations/pull/52)

## Context

Request lines are what TLS-intercepting proxies and access logs commonly record, and this layer trusts the platform certificate store, so such middleboxes can be reading. One adapter sent its key in the URL query.

## Decision

Every adapter sends its key in a request header. The Vertex express key moved to `x-goog-api-key`. Keys are checked at construction to hold no control characters or whitespace, so they cannot inject a header.

## Consequences

A new adapter must keep its key out of the URL. Redirects stay disabled for keyed requests.
