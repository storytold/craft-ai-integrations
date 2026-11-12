# ADR 0007: A person's model folders are kept as a map, never reorganised

- **Status:** accepted 17/10/2026
- **Source:** [#40](https://github.com/storytold/craft-ai-integrations/pull/40)

## Context

People already keep models in folders of their own, in whatever structure they like. Assuming ComfyUI's layout meant reorganising files or downloading a second copy.

## Decision

The store keeps a map of the person's folders and what is known to be in each (`model_folders`). Scanning is read only and bounded. A download never goes into a folder read from another program. The map can be written as a ComfyUI `extra_model_paths.yaml` for a ComfyUI the suite starts.

## Consequences

No file is moved or renamed. Older store files load unchanged, and an empty list is not written, so older apps still read the file.
