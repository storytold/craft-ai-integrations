# ADR 0025: Resume a cut download only against a named file version

- **Status:** accepted 11/11/2026
- **Source:** [#171](https://github.com/storytold/craft-ai-integrations/pull/171)

## Context

Large hosted results are sometimes cut part way, and signed links stop working after a while. Restarting from the first byte every time could mean a large file never finishes.

## Decision

A cut copy resumes with `Range` only when the host named the file's version, sent back as `If-Range`: a strong `ETag`, or, only when the answer carries no `ETag` at all, a `Last-Modified` at least 60 seconds before the response's own `Date`. A weak or malformed `ETag` means no version, with no fall back to `Last-Modified`. A `206` counts only when its `Content-Range` starts exactly at the kept copy's end. A link clearly past its signed expiry (more than `CLOCK_GRACE_SECS` late) is refused before any request.

## Consequences

A host whose file changed sends the whole new file instead of a mismatched piece. `PayloadHandle::expires_at` is optional and skipped when unset, so older payloads read as before.
