# Craft AI integrations: agent entry point

Read `CONTRIBUTING.md`, `README.md`, `docs/LAYOUT.md` and the relevant adopting guide before
editing. For a developer suggestion about a model, connection, engine or workflow, start at
`docs/PROPOSE-AN-INTEGRATION.md` and use `tools/intake/propose.py`. A suggestion is not a built-in
recommendation, a working integration or a user account. Keep credentials and user bindings out
of proposals and version control.

The repository's shared crates own the contracts, adapters, catalogue, consent and client.
Each Craft app owns its own interface, document conversion and result storage. Check the live
catalogue, route implementation and app adoption before claiming a provider or model is missing
or works. Never replace the selected model silently. Use targeted tests and scripted services;
live or paid calls, downloads, publication and merges need separate authorisation.
