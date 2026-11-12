# The MCP gateway to the Craft apps (`craft-ai-mcp`)

One MCP server through which an agent (Claude Desktop, Claude Code, Cursor, LM Studio, ...)
reaches every installed Craft app.

Each Craft app's CLI already serves MCP (`<app>-cli mcp`). Registering every app separately means
one client entry per app, and each lists all of its tools up front: PhotoCraft alone lists 26 tools,
34,446 bytes of definitions. The gateway is one entry with four tools (1,530 bytes):

| Tool | What it does |
|---|---|
| `craft_list_apps` | The apps available here; starts nothing. |
| `craft_find_tools` | Tools matching a few words (name and one-line summary, no schemas), in one app or all. With no match it names the tools that list or run an app's own engine commands (PhotoCraft's `command_list` and `command_run`). |
| `craft_describe_tool` | One tool's full definition with its input schema. |
| `craft_call_tool` | Runs one tool and returns the app's own result unchanged (text, images, `isError`). |

Tracked in #195 (EPIC-MCP-GATEWAY).

## How it works

- **On demand.** An app's MCP server is started the first time an agent needs that app (no console
  window on Windows) and kept for the life of the gateway. Its tool list is read once, and again
  after the app sends `notifications/tools/list_changed`, stops or is replaced.
- **Bounded.** Messages from an app are capped at 64 MiB per line and wait in a bounded queue;
  writes to it go through their own bounded queue. A start, a listing and a call each have a
  deadline (60 s, 30 s, and 600 s by default for a call, at most 3600 s). An app that misses one is
  dropped and started again on the next call; a call is never sent twice.
- **In parallel.** Requests from the client are answered in parallel (up to 16), so a long call
  does not hold up `ping` or a search, and a search over all apps starts them at once.
- **Requests from an app.** `ping` gets an empty result; anything else (sampling, roots) is
  answered as not supported.

## Which apps

The ArtCraft Launcher writes the installed apps into `ArtCraft/mcp/apps.json` in the per-user
local configuration folder (`%LOCALAPPDATA%\ArtCraft\mcp` on Windows, beside the AI layer's
`ArtCraft/ai`; `CRAFT_MCP_DIR` moves it):

```json
{ "version": 1, "apps": [
  { "id": "photocraft", "name": "PhotoCraft",
    "command": "C:/Program Files/PhotoCraft/photocraft-cli.exe", "args": ["mcp"] } ] }
```

`command` must be an absolute path; `args` defaults to `["mcp"]`. Rows with a bad id, a relative
command or a repeated id are left out, and `craft_list_apps` says why. Without the file, the
gateway looks for each known app's `<id>-cli` on `PATH`.

## Running it

```sh
craft-ai-mcp --stdio
```

That is the only mode: MCP over stdin and stdout, the way desktop clients start a local server.
There is no network listener, so nothing else on the computer can reach it. A client entry looks
like this (the launcher writes it, see below):

```json
{ "mcpServers": { "craft-apps": { "command": "C:/.../craft-ai-mcp.exe", "args": ["--stdio"] } } }
```

The command line is also a library function, `craft_ai_mcp::cli::main(args)`: the ArtCraft
Launcher builds its own `craft-ai-mcp` binary with it and ships it beside itself.

## The launcher

Settings › Experimental features › **AI agents (MCP)** (the `AI-MCP` flag, `craft-flags`) shows
Settings › **AI agents**: one switch that writes `apps.json` and copies `craft-ai-mcp` into
`ArtCraft/mcp/bin`, and one Add / Remove button per MCP client found on the computer. See
storytold/craft-launcher#1432.

## Modules

| Module | What it does |
|---|---|
| `apps/registry.rs` | Which apps: the launcher's `apps.json`, or the known `<id>-cli` names on `PATH`. |
| `apps/child.rs` | An MCP client for one app's `<app>-cli mcp`, with deadlines, queues and line caps. |
| `apps/mod.rs` | The hub: starts an app on first use, keeps its tool list. |
| `apps/search.rs` | Ranking an app's tools against a few words. |
| `apps/tools.rs` | The four tools: definitions and calls. |
| `mcp.rs` | MCP's JSON-RPC methods: `initialize`, `ping`, `tools/list`, `tools/call`. |
| `stdio.rs` | One JSON-RPC message per line on stdin and stdout (lines capped at 96 MiB). |
| `cli.rs` | The command line, `craft_ai_mcp::cli::main`. |

## Dependencies

`serde`, `serde_json` and `dirs`. None of the AI layer's crates: the gateway only forwards to the
apps' own MCP servers, and each app's AI features stay behind that app's own tools.

## Testing

- `cargo test -p craft-ai-mcp`: the gateway against in-process fake app servers (handshake, paging,
  log lines on stdout, requests from the app, error replies, a silent or exiting app, a failed
  write, a restart, a changed tool list, bad arguments), the JSON-RPC methods and the stdio loop.
- Deliberate-bug checks: `python tools/mutation/mutation_check.py mg`.
- By hand: put an `apps.json` in a folder, set `CRAFT_MCP_DIR` to it, and send JSON-RPC lines to
  `craft-ai-mcp --stdio`.
