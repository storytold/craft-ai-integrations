# Craft AI MCP Bridge (`craft-ai-mcp`)

A small Rust binary that exposes the Craft AI tool catalogue as **MCP tools** so that any
MCP-capable host — LM Studio, Bionic, or a future client — can call every built-in feature
(`generate-image`, `remove-background`, `generative-fill`, …) through one shared endpoint.

It is the "bridge" between the Craft AI apps and an LLM agent: the agent sees plain MCP tools;
the bridge routes each call into the same `AiClient` job path every app uses (status → build →
plan → consent → start → wait).

Over stdio it is also the **gateway to the Craft apps**: one MCP entry that reaches every installed
app's own MCP server through four small tools, instead of one entry per app that each lists all of
its tools (see [The gateway to the apps](#the-gateway-to-the-apps)).

## Why it exists

- The 18 Craft AI apps each ship their own GUI, but an LLM host has no way to drive them.
- This crate gives hosts a **single, stable surface** (`/mcp`) that lists every built-in tool
  with a JSON Schema and routes calls through the shared client — so a new app or tool added to
  the shared catalogue is exposed automatically, without touching any host config by hand.

## Architecture (module responsibilities)

| Module | Responsibility |
|---|---|
| `main.rs` | Entry point. `--stdio` serves MCP on stdin/stdout with the app gateway (`--no-apps` leaves it out); without a flag it serves HTTP on `CRAFT_MCP_PORT` (default **12345**) with the AI tools only. Builds the client with `AiClient::standard` (the shared store, and keys from the environment, then the OS keychain). |
| `stdio.rs` | MCP over stdin/stdout, one JSON-RPC message per line (lines capped at 96 MiB). Requests are answered in parallel (up to 16), so a long tool call does not hold up `ping` or a search. |
| `apps/` | The gateway to the Craft apps: `registry.rs` (which apps, from the launcher's `apps.json` or `PATH`), `child.rs` (an MCP client for one app's `<app>-cli mcp`, with deadlines and line caps), `search.rs` (ranking tools by a few words), `mod.rs` (the hub: starts an app on first use, keeps its tool list), `tools.rs` (the four gateway tools). |
| `server.rs` | Raw-TCP streamable-HTTP transport: one thread per connection, reads a single request (headers + body), dispatches to `McpState::handle`, writes a plain-JSON response with `Connection: close`. No HTTP framework on purpose — keeps the binary tiny and dependency-free. |
| `mcp.rs` | Protocol layer only. Maps MCP `initialize` / `tools/list` / `ping` / `tools/call` onto the shared catalogue and the router. Knows nothing about sockets or HTTP. Builds JSON Schemas for tool inputs from each feature's declared inputs. |
| `router.rs` | The heart of the bridge. Turns one MCP `tools/call` into a Craft AI job: looks up the feature, checks `AiClient::status` (the plain-language "what is missing" signal), builds the request from JSON args, plans it, honors consent, then starts and waits on the job. Returns `Succeeded` / `Unavailable` / `NeedsConsent` / `Failed`. |
| `catalogue.rs` | Normalizes the shared `craft_ai_features::builtin()` catalogue into flat `ToolDescriptor`s (id, title, summary, capability, per-app status) that the MCP server lists. |
| `discovery.rs` | Fetches the `storytold` org's public GitHub repos so newly landed apps can be noticed; not yet wired into `tools/list`, so a new app does not reach MCP hosts until it is added to the shared catalogue. Bounded reads (4 MiB), archived repos excluded, name must contain "craft". |
| `installer.rs` | Two installers: (1) merges this bridge's entry into Bionic's `ng-mcp.json` (idempotent; preserves other servers — a missing file is created fresh, but an unreadable existing file, invalid JSON, a non-object root, or a non-array `servers` value returns an error and leaves the file unchanged); (2) generates one `SKILL.md` folder per built-in feature for installation through Bionic's own `skill.install` tool. |

## Running the bridge

```sh
# stdio, with the gateway to the apps: what an MCP client starts
./target/debug/craft-ai-mcp --stdio

# HTTP on http://127.0.0.1:12345/mcp, AI tools only
./target/debug/craft-ai-mcp
CRAFT_MCP_PORT=8901 ./craft-ai-mcp
```

- **Localhost only.** The HTTP listener binds `127.0.0.1`. It has no caller authentication yet, so
  it offers the AI tools only and never the app gateway. Over stdio the client owns the process.
- **The shared AI setup.** Connections come from the shared store, and keys from the environment,
  then the OS keychain (`AiClient::standard`). `CRAFT_AI_DIR` moves the store (tests, portable).
- **Consent is never granted by the bridge.** A tool that would send data off the machine returns
  the disclosure; a person approves in the app or launcher. `CRAFT_MCP_AUTO_CONSENT=1` grants it
  without asking, for trusted local development only.

## The gateway to the apps

Each Craft app's CLI already serves MCP (`<app>-cli mcp`). Registering every app separately means
one client entry per app, and each lists all of its tools up front: PhotoCraft alone lists 26
tools, 34,446 bytes of definitions. `craft-ai-mcp --stdio` offers four tools instead (1,530 bytes):

| Tool | What it does |
|---|---|
| `craft_list_apps` | The apps available here; starts nothing. |
| `craft_find_tools` | Tools matching a few words (name and one-line summary, no schemas), in one app or all. With no match it names the tools that list or run an app's own engine commands (PhotoCraft's `command_list` and `command_run`). |
| `craft_describe_tool` | One tool's full definition with its input schema. |
| `craft_call_tool` | Runs one tool and returns the app's own result unchanged (text, images, `isError`). |

An app's MCP server is started the first time an agent needs that app and kept for the life of the
gateway; its tool list is read once. A server that does not answer within the deadline is dropped
and started again on the next call. A call is never sent twice.

**Which apps.** The ArtCraft Launcher writes the installed apps into `ArtCraft/mcp/apps.json` in
the per-user local configuration folder (`%LOCALAPPDATA%\ArtCraft\mcp` on Windows, beside the AI
layer's `ArtCraft/ai`; `CRAFT_MCP_DIR` moves it):

```json
{ "version": 1, "apps": [
  { "id": "photocraft", "name": "PhotoCraft",
    "command": "C:/Program Files/PhotoCraft/photocraft-cli.exe", "args": ["mcp"] } ] }
```

`command` must be an absolute path; `args` defaults to `["mcp"]`. Rows with a bad id, a relative
command or a repeated id are left out, and `craft_list_apps` says why. Without the file, the gateway
looks for each known app's `<id>-cli` on `PATH`.

**Client entry.** One entry for every app, for example in Claude Desktop or Cursor:

```json
{ "mcpServers": { "craft": { "command": "C:/.../craft-ai-mcp.exe", "args": ["--stdio"] } } }
```

## Connecting from Bionic / LM Studio

There are two complementary ways the apps reach a host:

### 1. The unified bridge (one entry, all built-in tools)

A single `streamableHttp` server in `ng-mcp.json`:

```json
{
  "id": "c55a52b4-6eca-44ac-926c-f67cceac9f1f",
  "name": "Craft AI",
  "description": "Craft AI image and video tools exposed as MCP.",
  "enabled": true,
  "connection": {
    "type": "streamableHttp",
    "url": "http://127.0.0.1:12345/mcp",
    "headers": {},
    "auth": { "type": "none" }
  }
}
```

`installer.rs::install_mcp_server` writes/updates exactly this entry (idempotent by `SERVER_ID`).
Bionic's schema requires both `headers` and `auth` on every streamableHttp connection; omitting
either rejects the whole file.

### 2. Per-app stdio CLIs (one entry per installed app)

Each Craft AI app ships a headless CLI with an `mcp` subcommand that speaks MCP over **stdio**.
Register each installed one in `ng-mcp.json`:

```json
{
  "id": "<uuid>",
  "name": "WordCraft",
  "description": "Document editing tools from Craft AI.",
  "enabled": true,
  "connection": {
    "type": "stdio",
    "command": "C:\\Programs\\WordCraft\\wordcraft-cli.exe",
    "args": ["mcp"],
    "env": {}
  }
}
```

The installed set (Windows paths; adjust per platform):

| App | stdio command |
|---|---|
| CADCraft | `C:\Programs\CADCraft\cadcraft-cli.exe mcp` |
| Craft Anything | `C:\Programs\Craft Anything\craft-anything-cli.exe mcp` |
| DeckCraft | `C:\Programs\DeckCraft\deckcraft-cli.exe mcp` |
| DesignCraft | `C:\Programs\DesignCraft\designcraft-cli.exe mcp` |
| EffectCraft | `C:\Programs\EffectCraft\effectcraft-cli.exe mcp` |
| FigCraft | `C:\Programs\FigCraft\figcraft-cli.exe mcp` |
| FilmCraft | `C:\Programs\FilmCraft\filmcraft-cli.exe mcp` |
| GridCraft | `C:\Programs\GridCraft\gridcraft-cli.exe mcp` |
| LightCraft | `C:\Programs\LightCraft\lightcraft-cli.exe mcp` |
| PdfCraft | `C:\Programs\PdfCraft\pdfcraft-cli.exe mcp` |
| PhotoCraft | `C:\Programs\PhotoCraft\photocraft-cli.exe mcp` |
| SolidCraft | `C:\Programs\SolidCraft\solidcraft-cli.exe mcp` |
| SoundCraft | `C:\Programs\SoundCraft\soundcraft-cli.exe mcp` |
| TextureCraft | `C:\Programs\TextureCraft\texturecraft-cli.exe mcp` |
| VectorCraft | `C:\Programs\VectorCraft\vectorcraft-cli.exe mcp` |
| WordCraft | `C:\Programs\WordCraft\wordcraft-cli.exe mcp` |
| MailCraft (beta) | `C:\Program Files\MailCraft\mailcraft-cli.exe mcp` |
| MaterialCraft | `C:\Programs\MaterialCraft\materialcraft-cli.exe mcp` |

To add a new app: drop its entry into `ng-mcp.json` using the same shape. No code change is needed
for the bridge itself — it lists whatever is in the shared catalogue, and discovery flags apps that
are on GitHub but not yet catalogued.

## Starting the HTTP bridge

Over stdio the MCP client starts and stops the bridge. The HTTP bridge must already be listening when
a host connects, so it is started on purpose: by hand, or from the ArtCraft Launcher's switch. It is
never installed as a logon task or started hidden.

### Verifying it is up

```sh
# Windows: a LISTENING socket on 127.0.0.1:12345 means the bridge is running
netstat -ano | findstr ":12345"

# Optional deeper check - MCP initialize handshake (expect protocolVersion, serverInfo,
# and capabilities.tools in the result):
curl -s -X POST http://127.0.0.1:12345/mcp -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{}}}'
```

Once the bridge is running, **restart Bionic** so it re-reads `ng-mcp.json` and connects to the Craft AI entry.

## Dynamic discovery of new apps

`discovery.rs::fetch_org_repos("storytold")` pulls the org's public repos from the GitHub API and
`craft_apps()` keeps those whose name contains "craft" (case-insensitive), excluding archived ones.
`unknown_apps(catalogue, org)` returns apps present in the org but absent from the shared catalogue —
the "prompt to add them" signal. This is read-only; it never writes to GitHub.

## Skill generation

`installer.rs::write_skills(base_dir, features)` writes one folder per built-in feature:
`<base_dir>/craft-ai-<feature-id>/SKILL.md`. The front matter (`name`, `display-name`,
`description`, `user-invocable`) follows Bionic's skill schema; the body tells the agent to use the
`craft-ai-mcp` MCP server's `<feature-id>` tool and lists the JSON arguments it accepts.

**Install through Bionic's own `skill.install` tool** (it validates and asks for approval). Do not copy
folders into the global skills directory by hand — see the skill-management guidance.

## Testing

- **Unit / integration tests:** `cargo test -p craft-ai-mcp --lib` (and the crate-level tests in
  `tests/`). They use a client with no saved connections and an empty key store, so every remote
  tool reports "what is missing" — exactly the path we want to exercise. No real provider is contacted.
- **Manual stdio smoke test** (one app):

  ```sh
  printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}\n' \
    | 'C:\Programs\WordCraft\wordcraft-cli.exe' mcp
  ```

  A healthy server replies with a `result` containing `protocolVersion`, `capabilities.tools`, and
  `serverInfo`. Then send `tools/list` (returns every built-in tool) and, if a model is bound,
  `tools/call`.

## Known issues & limitations

- **Four CLIs reject the initialize handshake with newer protocol revisions.** (2026-11-12: a
  `photocraft-cli` built from PhotoCraft's `main` completed `initialize` from the gateway without
  `_meta`, so this may no longer hold for PhotoCraft; the other three were not rechecked.) As first written,
  `FilmCraft`, `PhotoCraft`, `TextureCraft`, and `MaterialCraft` use an MCP server that (a) returns
  JSON-RPC error `-32602` ("request _meta is missing or has malformed required fields") when the
  initialize request lacks `_meta`, but then (b) returns `-32601` ("initialize" / method not found)
  when `_meta` *is* present inside `params`. The two requirements are mutually unsatisfiable, so a
  standard MCP client cannot complete initialize against them. This is a bug in those CLIs' recent
  updates (they moved to a revision that requires `_meta` but broke the stdio initialize path).
  **Action:** report to the Craft AI team; until fixed, these four will not connect through Bionic's
  stdio MCP. The other 14 stdio CLIs and the unified HTTP bridge are unaffected.
- **Protocol version negotiation.** The bridge negotiates on `initialize`: it echoes a client's
  requested revision when that revision is one the bridge has been tested against (`2025-06-18`,
  `2025-11-25`), and otherwise falls back to `2025-06-18`. It never advertises an unpublished
  revision (the previously hardcoded `2025-03-07` was rejected by LM Studio). Revisions that require
  `_meta` inside `params` (`2025-11-25`, `2026-07-28`) are a client-side concern; Bionic's client
  must be checked against this.
- **Click/reference inputs not expressible.** App-specific inputs (e.g. a selection tool's clicks)
  cannot be passed through plain JSON args in v1; the router rejects them with a clear message.
- **Localhost only.** The bridge binds `127.0.0.1`; it is not intended to be exposed on a network.
- **Bionic rejects the whole `ng-mcp.json` if any single entry is schema-invalid.** It does not skip bad entries; one malformed server (e.g. a `streamableHttp` entry missing its required `auth` block) causes *all* servers to fail to load. Validate each entry against Bionic's expectations before adding it.
- **`ng-mcp.json` must match Bionic's native serialization: no UTF-8 BOM, LF line endings, 2-space indent.** A leading BOM or CRLF/reformatted whitespace makes Bionic reject the entire file (zero servers load). Regenerate with `json.dumps(obj, indent=2)` written as LF to be safe.
- **`streamableHttp` entries require an `auth` object.** Bionic's schema requires both `headers` and an `auth` object on every streamableHttp connection; omitting either rejects the whole file. The bridge uses `auth.type: "none"` for its localhost endpoint, which is accepted — no bearer token is needed.

## Security notes

- Consent is honored exactly as in the apps: local connections never prompt; remote destinations need
  an agreement that a person gave in the app or launcher. The bridge never grants it itself
  (unless `CRAFT_MCP_AUTO_CONSENT=1` is set for development). The router surfaces `NeedsConsent` with a disclosure naming *kinds and sizes* of data
  that would leave the machine — never contents.
- Discovery reads only public GitHub metadata; it sends no keys or secrets.
- The app gateway is offered over stdio only, where the MCP client owns the process. It starts only
  the absolute commands the launcher listed (or the known `<id>-cli` names on `PATH`), without a
  shell, and each app's own rules (its folder limits, its consent) still apply to what its tools do.
