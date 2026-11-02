# owl-shell-tool-mcp

**Note: This MCP server is still experimental. When using it with owl, ensure the CLI version matches the MCP server version.**

`owl-shell-tool-mcp` is an MCP server that provides a tool named `shell` that runs a shell command inside a sandboxed instance of Bash. This special instance of Bash intercepts requests to spawn new processes (specifically, [`execve(2)`](https://man7.org/linux/man-pages/man2/execve.2.html) calls). For each call, it makes a request back to the MCP server to determine whether to allow the proposed command to execute. It also has the option of _escalating_ the command to run unprivileged outside of the sandbox governing the Bash process.

The user can use owl's `.rules` files (see [codex-rs/execpolicy/README.md](../codex-rs/execpolicy/README.md)) to define how a command should be handled. The action to take is determined by the `decision` parameter of a matching rule as follows:

- `allow`: the command will be _escalated_ and run outside the sandbox
- `prompt`: the command will be subject to human approval via an [MCP elicitation](https://modelcontextprotocol.io/specification/draft/client/elicitation) (it will run _escalated_ if approved)
- `forbidden`: the command will fail with exit code `1` and an error message will be written to `stderr`

Commands that do not match an explicit rule in `.rules` will be allowed to run as-is, though they will still be subject to the sandbox applied to the parent Bash process.

## Motivation

When a software agent asks if it is safe to run a command like `ls`, without more context, it is unclear whether it will result in executing `/bin/ls`. Consider:

- There could be another executable named `ls` that appears before `/bin/ls` on the `$PATH`.
- `ls` could be mapped to a shell alias or function.

Because `owl-shell-tool-mcp` intercepts `execve(2)` calls directly, it _always_ knows the full path to the program being executed. In turn, this makes it possible to provide stronger guarantees on how owl's `.rules` are enforced.

## Usage (build from source)

owl does not ship prebuilt binaries or npm packages: build the pieces from this repository.

```bash
cd owl/codex-rs
cargo build -p codex-exec-server   # codex-exec-mcp-server + codex-execve-wrapper
```

Build the patched Bash with the patch bundled in this repo (see the "Patched
Bash" section of [codex-rs/exec-server/README.md](../codex-rs/exec-server/README.md)),
then launch the MCP server with both paths:

```bash
codex-rs/target/debug/codex-exec-mcp-server \
  --execve codex-rs/target/debug/codex-execve-wrapper \
  --bash /path/to/patched/bash
```

To try it out with owl in one shot, disable the built-in shell tool so owl has
exactly one shell-like tool available to it:

```bash
owl --disable shell_tool \
  --config 'mcp_servers.shell-tool={command = "/absolute/path/to/codex-exec-mcp-server", args = ["--execve", "/absolute/path/to/codex-execve-wrapper", "--bash", "/absolute/path/to/patched/bash"]}'
```

To configure it permanently so you can run `owl` without additional flags, add
the same entry to your `~/.owl/config.toml`:

```toml
[features]
shell_tool = false

[mcp_servers.shell-tool]
command = "/absolute/path/to/codex-exec-mcp-server"
args = ["--execve", "/absolute/path/to/codex-execve-wrapper", "--bash", "/absolute/path/to/patched/bash"]
```

## MCP Client Requirements

This MCP server is designed to be used with owl, which acts as an MCP client and supports the following `capability`:

```json
{
  "capabilities": {
    "experimental": {
      "codex/sandbox-state": {
        "version": "1.0.0"
      }
    }
  }
}
```

This capability means the MCP server honors requests like the following to update the sandbox policy the MCP server uses when spawning Bash:

```json
{
  "id": "req-42",
  "method": "codex/sandbox-state/update",
  "params": {
    "sandboxPolicy": {
      "type": "workspace-write",
      "writable_roots": ["/home/user/code/my-project"],
      "network_access": false,
      "exclude_tmpdir_env_var": false,
      "exclude_slash_tmp": false
    }
  }
}
```

Once the server has processed the update, it sends an empty response to acknowledge the request:

```json
{
  "id": "req-42",
  "result": {}
}
```

owl's harness (interactive mode and `owl exec`) sends such requests to MCP servers that declare the `codex/sandbox-state` capability. (The capability name is part of the wire protocol and stays as-is.)

## Launcher script

`pnpm build` in this folder generates `bin/mcp-server.js`, a convenience
launcher that picks binaries from a staged `vendor/` tree based on the current
`process.platform` / `process.arch`. owl ships no `vendor/` tree; if you use
the launcher, stage your own `codex-exec-mcp-server`, `codex-execve-wrapper`,
and patched Bash builds under `vendor/<target-triple>/`. Launching the Rust
binaries directly (above) is the supported path.

See [the README in codex-rs/exec-server](../codex-rs/exec-server/README.md) for details on the binaries themselves.
