# Owl

**Owl** is a terminal agent for **your own self-hosted [Open WebUI](https://github.com/open-webui/open-webui)
server**. It is a fork of the open-source [`openai/codex`](https://github.com/openai/codex) CLI
(Rust `codex-rs`, pinned to tag `rust-v0.90.0`) retargeted to talk to the OpenAI-compatible API your
Open WebUI serves (`/api/v1`) — chat completions, and the model list straight from your server.

There is no account to sign into, no plan, no pricing, no credits: `owl` only ever talks to the
server URL you configure. On first run it asks for that URL (and an optional API token) on a
Connect screen; after that it just works.

```text
  ,_,
 (o,o)    ready when you are.
 (   )
  " "
```

## Install

`install.sh` builds `owl` from source and puts it on your `PATH` (needs a Rust toolchain; Linux
and macOS, or Windows under WSL2):

```sh
cd owl
./install.sh                                      # release build -> ~/.local/bin/owl
./install.sh --base-url http://your-server:8080/api/v1   # also save the server in config.toml
```

Or build manually: `cd codex-rs && cargo build -p codex-cli --bin owl`.

## Run

```sh
owl                    # interactive TUI; first run shows the Connect screen
owl exec "..."         # one-shot from the command line
```

Or set the server up front instead of using the Connect screen:

```sh
export OPENWEBUI_BASE_URL="http://your-server:8080/api/v1"
export OPENWEBUI_API_KEY="<optional>"
```

Settings live in `~/.owl/` (override with `OWL_HOME`). The model picker lists exactly what your
server serves.

## Documentation

- **[README-openwebui.md](README-openwebui.md)** — the full guide: configuration, providers,
  what this fork changed and why, and how the fork's history is kept.
- [docs/](docs/) — additional upstream-derived docs (install, config, contributing).

## License

Apache-2.0 — see [LICENSE](LICENSE). Owl is derived from
[openai/codex](https://github.com/openai/codex); the fork keeps upstream's license and
notices, but ships none of its account, login, model-catalog, pricing or telemetry machinery.
