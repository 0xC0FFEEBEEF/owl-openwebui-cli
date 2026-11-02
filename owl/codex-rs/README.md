# owl (Rust implementation)

owl is a terminal coding agent that runs entirely against your own Open WebUI server. It ships as a standalone, native executable built from source - there is no npm package, no hosted account, and no bundled model.

## Installing owl

Build and install from this repository:

```shell
cd owl && ./install.sh
```

Or, from this folder (the Cargo workspace root):

```shell
cargo build
cargo run --bin owl
```

See [`docs/install.md`](../docs/install.md) for toolchain details and [`README-openwebui.md`](../README-openwebui.md) for the first-run connect walkthrough.

## Documentation quickstart

- First run with owl? Start with [`docs/getting-started.md`](../docs/getting-started.md) (links to the walkthrough for prompts, keyboard shortcuts, and session management).
- Want deeper control? See [`docs/config.md`](../docs/config.md) and [`docs/install.md`](../docs/install.md).

## What's in the Rust CLI

### Config

owl supports a rich set of configuration options. The settings home is `~/.owl` (overridable with `OWL_HOME`) and the file is `config.toml`. Your Open WebUI server lives under `[model_providers.openwebui]`. See [`docs/config.md`](../docs/config.md) for details.

### Model Context Protocol Support

#### MCP client

owl functions as an MCP client: MCP servers you define under `[mcp_servers]` in `config.toml` are available in both the interactive TUI and `owl exec`. See the [`configuration documentation`](../docs/config.md#connecting-to-mcp-servers) for details.

#### MCP server (experimental)

owl can be launched as an MCP _server_ by running `owl mcp-server`. This allows _other_ MCP clients to use owl as a tool for another agent.

Use the [`@modelcontextprotocol/inspector`](https://github.com/modelcontextprotocol/inspector) to try it out:

```shell
npx @modelcontextprotocol/inspector owl mcp-server
```

Use `owl mcp` to add/list/get/remove MCP server launchers defined in `config.toml`, and `owl mcp-server` to run the MCP server directly.

### Notifications

You can enable notifications by configuring a script that is run whenever the agent finishes a turn. The [notify documentation](../docs/config.md#notify) includes a detailed example that explains how to get desktop notifications via [terminal-notifier](https://github.com/julienXX/terminal-notifier) on macOS. When owl detects that it is running under WSL 2 inside Windows Terminal (`WT_SESSION` is set), the TUI automatically falls back to native Windows toast notifications so approval prompts and completed turns surface even though Windows Terminal does not implement OSC 9.

### `owl exec` to run owl programmatically/non-interactively

To run owl non-interactively, run `owl exec PROMPT` (you can also pass the prompt via `stdin`) and owl will work on your task until it decides that it is done and exits. Output is printed to the terminal directly. You can set the `RUST_LOG` environment variable to see more about what's going on.

### Experimenting with the owl Sandbox

To test to see what happens when a command is run under the sandbox provided by owl, we provide the following subcommands:

```
# macOS
owl sandbox macos [--full-auto] [--log-denials] [COMMAND]...

# Linux
owl sandbox linux [--full-auto] [COMMAND]...

# Windows
owl sandbox windows [--full-auto] [COMMAND]...

# Legacy aliases
owl debug seatbelt [--full-auto] [--log-denials] [COMMAND]...
owl debug landlock [--full-auto] [--log-denials] [COMMAND]...
```

### Selecting a sandbox policy via `--sandbox`

The Rust CLI exposes a dedicated `--sandbox` (`-s`) flag that lets you pick the sandbox policy **without** having to reach for the generic `-c/--config` option:

```shell
# Run owl with the default, read-only sandbox
owl --sandbox read-only

# Allow the agent to write within the current workspace while still blocking network access
owl --sandbox workspace-write

# Danger! Disable sandboxing entirely (only do this if you are already running in a container or other isolated env)
owl --sandbox danger-full-access
```

The same setting can be persisted in `~/.owl/config.toml` via the top-level `sandbox_mode = "MODE"` key, e.g. `sandbox_mode = "workspace-write"`.

## Code Organization

This folder is the root of a Cargo workspace. It contains quite a bit of experimental code, but here are the key crates:

- [`core/`](./core) contains the business logic for owl. Ultimately, we hope this to be a library crate that is generally useful for building other Rust/native applications that use a self-hosted coding agent.
- [`exec/`](./exec) "headless" CLI for use in automation.
- [`tui/`](./tui) CLI that launches a fullscreen TUI built with [Ratatui](https://ratatui.rs/).
- [`cli/`](./cli) CLI multitool that provides the aforementioned CLIs via subcommands.
