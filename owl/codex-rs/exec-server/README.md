# codex-exec-server

This crate contains the code for two executables:

- `codex-exec-mcp-server` is an MCP server that provides a tool named `shell` that runs a shell command inside a sandboxed instance of Bash. Every resulting `execve(2)` call made within Bash is intercepted and run via the executable defined by the `BASH_EXEC_WRAPPER` environment variable within the Bash process. In practice, `BASH_EXEC_WRAPPER` is set to `codex-execve-wrapper`.
- `codex-execve-wrapper` is the executable that takes the arguments to the `execve(2)` call and "escalates" it to the MCP server via a shared file descriptor (specified by the `CODEX_ESCALATE_SOCKET` environment variable) for consideration. Based on owl's `.rules` files (see [execpolicy/README.md](../execpolicy/README.md)), the MCP server replies with one of:
  - `Run`: `codex-execve-wrapper` should invoke `execve(2)` on itself to run the original command within Bash
  - `Escalate`: forward the file descriptors of the current process to the MCP server so the command can be run faithfully outside the sandbox. Because the MCP server will have the original FDs for `stdout` and `stderr`, it can write those directly. When the process completes, the MCP server forwards the exit code to `codex-execve-wrapper` so that it exits in a consistent manner.
  - `Deny`: the MCP server has declared the proposed command to be "forbidden," so `codex-execve-wrapper` will print an error to `stderr` and exit with `1`.

## Patched Bash

We carry a small patch to `execute_cmd.c` (see [`shell-tool-mcp/patches/bash-exec-wrapper.patch`](../../shell-tool-mcp/patches/bash-exec-wrapper.patch)) that adds support for `BASH_EXEC_WRAPPER`. The original commit message is “add support for BASH_EXEC_WRAPPER” and the patch applies cleanly to `a8a1c2fac029404d3f42cd39f5a20f24b6e4fe4b` from https://github.com/bminor/bash. To rebuild manually:

```bash
git clone https://github.com/bminor/bash
git checkout a8a1c2fac029404d3f42cd39f5a20f24b6e4fe4b
git apply /path/to/shell-tool-mcp/patches/bash-exec-wrapper.patch
./configure --without-bash-malloc
make -j"$(nproc)"
```

## Building from source and wiring it into owl

`cargo build -p codex-exec-server` produces `codex-exec-mcp-server` and
`codex-execve-wrapper`. Build the patched Bash (section above), then launch the
MCP server with both paths:

```bash
codex-rs/target/debug/codex-exec-mcp-server \
  --execve codex-rs/target/debug/codex-execve-wrapper \
  --bash /path/to/patched/bash
```

To use it from owl, disable the built-in shell tool and register the server in
`~/.owl/config.toml`:

```toml
[features]
shell_tool = false

[mcp_servers.shell-tool]
command = "/absolute/path/to/codex-exec-mcp-server"
args = ["--execve", "/absolute/path/to/codex-execve-wrapper", "--bash", "/absolute/path/to/patched/bash"]
```

The integration tests under `tests/` need the same patched Bash; set
`OWL_PATCHED_BASH` to point at it (see `tests/suite/bash`).
