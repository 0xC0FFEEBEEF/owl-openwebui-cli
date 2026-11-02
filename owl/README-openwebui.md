# Owl CLI: a Codex CLI fork that works out-of-the-box with Open WebUI

This is a fork of the official [Codex CLI](https://github.com/openai/codex)
(`codex-rs`, based on tag `rust-v0.90.0`) modified so that the command — renamed **`owl`** —
talks to **your own Open WebUI server** instead of requiring an OpenAI login.

## Why this fork exists

Open WebUI exposes an OpenAI-compatible **chat completions** API
(`/api/v1/chat/completions`). Newer Codex releases (0.95+) removed support for
the chat-completions wire API entirely and only speak the OpenAI Responses API,
which Open WebUI does not implement. This fork is pinned to `rust-v0.90.0`, the
last release with native chat-completions support, and makes Open WebUI a
first-class, built-in provider.

## What was changed

1. **Built-in `openwebui` provider** (`core/src/model_provider_info.rs`):
   chat-completions wire API, no OpenAI auth required.
2. **It is the default provider, always** (`core/src/config/mod.rs`): unless
   `model_provider` in `config.toml` (or a config profile, or `--provider`) names a
   provider, `owl` uses `openwebui`. A ChatGPT login left in the config folder by an
   upstream `codex login`, an exported `OPENAI_API_KEY` or the ephemeral
   `CODEX_API_KEY` no longer steer `owl` onto OpenAI - that inference is what made
   `owl` skip the Connect screen on a machine that used to run Codex. Ask for a
   different server explicitly instead (see "Other providers").
3. **Models come from your server** (`core/src/models_manager/manager.rs`):
   the model picker and default model are populated from
   `GET {base_url}/models` (the standard OpenAI model-listing endpoint Open
   WebUI serves), replacing — not merging with — the bundled ChatGPT catalog.
4. **No misleading deprecation notice**: upstream warns that `wire_api = "chat"`
   is deprecated; for this fork's `openwebui` provider that warning is
   suppressed because chat completions are the point.
5. **A Connect screen instead of a login** (`tui/src/onboarding/connect.rs`): on the
   first run — when the `openwebui` provider is selected and nothing is configured
   yet (no `[model_providers.openwebui]` in `config.toml`, no `OPENWEBUI_BASE_URL`)
   — `owl` asks for your **server** and an **optional API token**, and writes them to
   `~/.owl/config.toml` (`base_url`, `name = "Open WebUI"`, and the token as
   `experimental_bearer_token`; the token input is masked). A bare host like
   `owl.example.com` becomes `http://owl.example.com:8080/api/v1`. After saving, the
   screen is not shown again; edit `config.toml` (or the env vars) to change the server.
6. **The starting model comes from your server**: a fresh `owl` session resolves its
   model from `GET {base_url}/models` (bounded by a 5 s timeout, and the cached list
   answers within its 5-minute TTL) rather than from the bundled ChatGPT catalog, so
   the first request cannot name a model your server does not serve.
7. **No upstream update check**: `check_for_update_on_startup` defaults to **false**
   (`core/src/config/mod.rs`), so `owl` never asks `api.github.com` whether a newer
   `openai/codex` release exists and the "Update available! 0.90.0 -> 0.x" popup that
   links to `https://github.com/openai/codex` never appears. `owl` is built from
   source and has no upstream release channel; set `check_for_update_on_startup = true`
   in `config.toml` if you want the upstream check anyway.
8. **The account side of the codebase is deleted**: `owl login` / `owl logout` /
   `owl apply` / `owl cloud` and their crates (the ChatGPT auth screens, the ChatGPT
   "apps"/connectors feature, the cloud-tasks client crates), and the built-in
   analytics exporter that shipped usage metrics to `ab.chatgpt.com` - the otel layer
   now has no default endpoint at all. Features that upstream tied to a ChatGPT
   account (review, compaction, personality, MCP tools) simply run on your configured
   provider's model; your own `[mcp_servers]` keep working.
9. **Web search runs on your box** (`core/src/tools/handlers/web_search.rs`):
   upstream `web_search` is a hosted Responses-API tool executed by ChatGPT's
   servers - on the chat wire it was silently dropped. In `owl`, turning on
   `web_search` offers the model an ordinary function tool and **owl itself** runs
   each query against your own `[search]` endpoint (for example a self-hosted
   SearXNG) and returns the results to the model. See
   [docs/config.md](docs/config.md#web-search).
10. **Tool calls finish reliably on the chat wire** (`codex-api/src/sse/chat.rs`):
    many OpenAI-compatible backends (Open WebUI's bridges included) stream tool
    calls in deltas but end the stream with `finish_reason: "stop"`, plain
    `[DONE]`, or a bare close. owl used to dispatch calls only on an explicit
    `"tool_calls"` finish chunk, so an announced action could simply never
    happen - the turn ended quietly after the announcement. Buffered calls are
    now dispatched on every end-of-stream path. A call whose arguments were cut
    off mid-stream (the server hit an output-token limit) is handed to core as
    well: core answers it with a `failed to parse function arguments` tool
    result, so the model sees the cut-off and can retry in smaller pieces
    instead of the turn stopping silently after the announcement.
11. **A prebuilt binary ships in the repo**: `prebuilt/owl-x86_64-unknown-linux-gnu`
    (with its `prebuilt/SHA256SUMS`) is built from this folder's source on the build
    cluster, so a fresh clone's `./install.sh` puts `owl` on your PATH without compiling
    and without a Rust toolchain. `install.sh --force-build` ignores it and builds from
    source; if the bundled binary cannot run on your machine, `install.sh` says so and
    builds instead. See *Installing with `install.sh`* below.

## Quick start

Build (in `codex-rs/`):

```sh
cargo build -p codex-cli --bin owl     # binary: codex-rs/target/debug/owl
```

Point it at your Open WebUI server (pick one):

- **Environment variables** — `OPENWEBUI_BASE_URL` defaults to
  `http://localhost:8080/api/v1`:

  ```sh
  export OPENWEBUI_BASE_URL="http://your-server:8080/api/v1"
  export OPENWEBUI_API_KEY="<optional: your Open WebUI API key>"  # sent as a bearer token
  owl exec "summarize this repo"
  ```

- **Or `~/.owl/config.toml`**:

  ```toml
  # Optional: the built-in default already points at the openwebui provider.
  model = "qwen3:32b"        # any model id shown in your Open WebUI
  # model_provider = "openwebui"   # only needed if you also use other providers

  # Optional: fully override the built-in provider entry.
  [model_providers.openwebui]
  name = "Open WebUI"
  base_url = "http://your-server:8080/api/v1"
  env_key = "OPENWEBUI_API_KEY"     # optional; requests are unauthenticated without it
  wire_api = "chat"

  # Optional: let the model search the web through your own search endpoint.
  # web_search = "live"
  # [search]
  # url = "http://your-server:8888/search"   # e.g. self-hosted SearXNG
  # params = { format = "json" }
  ```

With no API key, requests are sent unauthenticated — fine for a local Open
WebUI where you're already signed in through the web UI (the key Open WebUI
shows under _Settings → Admin Settings → API Keys_ works as `OPENWEBUI_API_KEY`
if you want proper auth).

Notes:

- The `/model` picker and the default model list are fetched live from
  `{base_url}/models` and cached for 5 minutes in `~/.owl/models_cache.json`.
- For small local models, set `model_context_window` (tokens) in
  `~/.owl/config.toml` so Owl compacts conversations before your model
  runs out of context.
- `OPENWEBUI_PORT` can replace the port part of the default localhost URL.

## Installing with `install.sh`

`install.sh` puts `owl` on your PATH. This folder bundles a prebuilt release binary
(`prebuilt/`): when it matches your machine and runs, it is used right away - no compiling,
no Rust toolchain needed. Otherwise `install.sh` builds `owl` from source, which needs a Rust
toolchain (rustup); Linux and macOS work, Windows under WSL2 (see `docs/install.md` for the
system requirements).

```sh
./install.sh                                        # prebuilt when it runs here, else release build -> ~/.local/bin/owl
./install.sh --prefix /usr/local/bin                # somewhere else
./install.sh --base-url http://your-server:8080/api/v1   # also save the server in config.toml
./install.sh --force-build                          # ignore the prebuilt and compile from source
./install.sh --skip-build                           # reuse what is already in codex-rs/target/
./install.sh --debug                                # install the debug build instead
./install.sh --help
```

- The only file it installs is the `owl` binary. On Linux, Owl sandboxes shell commands by
  re-running itself with `arg0 = codex-linux-sandbox` (`codex-rs/arg0/src/lib.rs:100`), so no
  helper binary has to sit next to it.
- `--base-url` appends a `[model_providers.openwebui]` block to `~/.owl/config.toml`
  (`$OWL_HOME`; a legacy `$CODEX_HOME` is still honored if set) only when that section
  is not already there, and never stores an API key —
  use the connect screen or `OPENWEBUI_API_KEY` for that.
- Without `--base-url`, the first `owl` run shows the connect screen and saves the server itself.
- The bundled prebuilt covers **x86_64 Linux with glibc 2.39 or newer** (it is built on the
  build cluster, Ubuntu 24.04, and references `GLIBC_2.39` symbols). `install.sh` first checks
  the binary against `prebuilt/SHA256SUMS`, then smoke-runs it; if either step fails (older
  distro, different CPU, macOS), it falls back to building from source - same end result,
  just slower.
- The self-test (`scripts/install-selftest.sh`) proves both paths: the prebuilt install works
  with no `cargo` on `PATH` at all, and `--force-build` compiles the same tree from source.

## Other providers

`openwebui` is `owl`'s only built-in cloud-style provider; the upstream `openai`
built-in was removed with this fork, and no login command exists — a saved ChatGPT
login or an exported `OPENAI_API_KEY` cannot move `owl` off your server.

To point `owl` at a different OpenAI-compatible server, declare one in `config.toml`:

```toml
[model_providers.mygpu]
name = "My GPU box"
base_url = "http://gpu-box:11434/v1"
env_key = "MYGPU_API_KEY"        # optional bearer token
wire_api = "chat"
```

and select it with `model_provider = "mygpu"` (or `-c model_provider='"mygpu"'`).
The upstream ollama and lmstudio built-ins keep working, and user-defined provider
entries in `config.toml` override built-ins of the same id.

## Name

- The command is `owl` (upstream `codex` binary renamed; the Cargo package is
  still `codex-cli`, subcommand helper binaries like `codex-linux-sandbox` are
  unchanged).
- The mascot is a small ASCII owl (`tui/src/little_owl.rs`): it sits above the Welcome
  and Connect screens, and in a session a Thinking / "all done!" block appears above the
  status line while a task runs and for a few seconds after it finishes (it is cleared on
  an interrupted or failed turn). The upstream Codex welcome animation is gone.
- The `/status` card has no account rows at all: no plan, credits, or usage-limit
  lines, and a test (`tui/src/status/tests.rs`) keeps them from coming back.
- Config and state live in `~/.owl` (override with `OWL_HOME`; a legacy `CODEX_HOME`
  is still honored so old scripts keep working). `owl` never reads `~/.codex` by
  default, so a leftover Codex install - config, saved login, model cache - cannot
  affect it.

## Where the fork's own history lives (this machine)

What you push is the `owl/` folder: a plain snapshot of the working tree. The fork's commit
history sits next to it, in a git directory called `.owl-git/` in the folder that contains
`owl/`, and it is kept out of every commit (listed in that repo's `.git/info/exclude`) so it
never reaches GitHub. It cannot live _inside_ `owl/`: git treats any folder that contains a
`.git` directory as a separate repository (a submodule), which is why the fork's files are
tracked normally and its git data is kept beside them.

`.owl-git/` is a clone of [openai/codex](https://github.com/openai/codex) at the tag this fork
is pinned to, plus the fork's own commits on the branch `owui-fork`. Those commits are local to
this machine; the same code is in the `owl/` snapshot you push. Run these from the folder that
contains `owl/`:

```sh
git --git-dir=.owl-git --work-tree=owl log --oneline owui-fork             # the fork's commits
git --git-dir=.owl-git --work-tree=owl diff --stat rust-v0.90.0 owui-fork  # everything it changed
```

If `.owl-git/` is deleted nothing in `owl/` is lost. To get the comparison back, clone
`openai/codex` into a side folder, check out `rust-v0.90.0`, and compare its `codex-rs/` with
`owl/codex-rs/` (ignore `codex-rs/target/`, which is build output and never committed).
