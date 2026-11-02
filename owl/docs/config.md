# Configuration

Owl reads its settings from `~/.owl/config.toml` (set `OWL_HOME` to move them; a legacy
`CODEX_HOME` is still honored so old scripts keep working). Its only built-in network
provider is `openwebui` — your own Open WebUI server via the chat-completions API.
See [README-openwebui.md](../README-openwebui.md) for setup, environment variables, and
how to define your own provider.

## Basic settings

Useful top-level keys (all optional):

- `model` — a model id your server serves; when unset, owl takes the default from your
  server's `GET {base_url}/models` listing.
- `model_provider` — name a `[model_providers.*]` entry; the default is `openwebui`.
- `model_context_window` — token budget owl uses to compact conversations before a
  local model runs out of context.
- `check_for_update_on_startup` — defaults to `false` in this fork (owl has no upstream
  release channel).

## Web search

By default owl's model only sees your local workspace. To let it search the web as
well, turn on `web_search` and point it at your own search endpoint: owl runs each
query itself and hands the results back to the model as an ordinary tool result, so
this works with any model on the chat-completions wire and contacts no third-party
search service.

```toml
web_search = "live"                      # "cached" works too; "disabled" is the default

[search]
url = "http://127.0.0.1:8888/search"    # e.g. a self-hosted SearXNG instance
params = { format = "json" }            # SearXNG needs format=json; any extra query params
api_key_env = "SEARXNG_API_KEY"         # optional; sent as Authorization: Bearer <value>
max_results = 5                         # defaults to 5
timeout_ms = 15000                      # defaults to 15000
```

Owl sends `GET {url}?q=<query>&<params...>` and reads results from the response's
`web` or `results` array, accepting `title`, `url`/`link`, and
`content`/`description`/`snippet` fields. `owl --search` in the TUI is short for
`web_search = "live"`. If web search is enabled but `[search]` has no `url`, owl
reports that at startup instead of letting the tool fail mid-turn. (When you
configure a custom provider that speaks the responses wire, `cached`/`live` instead
map to that server's hosted search tool.)

## Connecting to MCP servers

Owl connects to MCP servers you declare in `[mcp_servers]` in `~/.owl/config.toml`,
using the same table shape as upstream Codex (`command`, `args`, `env`, `url`,
`bearer_token_env_var`, ...). Owl has no built-in or hosted apps/connectors beyond the
servers you configure yourself.

## Notify

Owl can run a notification hook when the agent finishes a turn — set `notify` in
`config.toml`.

## JSON Schema

The generated JSON Schema for `config.toml` lives at `codex-rs/core/config.schema.json`.

## Notices

Owl stores "do not show again" flags for some UI prompts under the `[notice]` table.

Ctrl+C/Ctrl+D quitting uses a ~1 second double-press hint (`ctrl + c again to quit`).
