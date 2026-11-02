## Installing & building

### System requirements

| Requirement                 | Details                                                         |
| --------------------------- | --------------------------------------------------------------- |
| Operating systems           | macOS 12+, Ubuntu 20.04+/Debian 10+, or Windows 11 **via WSL2** |
| Git (optional, recommended) | 2.23+ for built-in PR helpers                                   |
| RAM                         | 4-GB minimum (8-GB recommended)                                 |

### Install with `install.sh` (recommended)

```bash
git clone https://github.com/0xC0FFEEBEEF/Owl.git
cd Owl/owl && ./install.sh
```

`install.sh` puts `owl` into your PATH. The folder ships a prebuilt binary for x86_64 Linux
(glibc 2.39 or newer) and uses it right away when it runs on your machine - no compiling, no
Rust toolchain needed. If it does not run here, or you pass `--force-build`, `install.sh`
builds the `owl` binary from this folder (release profile) instead, which needs a Rust toolchain
(rustup); `./install.sh --help` lists its options. On first run owl asks for your Open WebUI
server URL and API token and saves them to `~/.owl/config.toml` (see
[README-openwebui.md](../README-openwebui.md)).

### Build from source

```bash
# Clone the repository and navigate to the root of the Cargo workspace.
git clone https://github.com/0xC0FFEEBEEF/Owl.git
cd Owl/owl/codex-rs

# Install the Rust toolchain, if necessary.
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustup component add rustfmt
rustup component add clippy
# Install helper tools used by the workspace justfile:
cargo install just
# Optional: install nextest for the `just test` helper
cargo install cargo-nextest

# Build owl.
cargo build

# Launch the TUI with a sample prompt.
cargo run --bin owl -- "explain this codebase to me"

# After making changes, use the root justfile helpers (they default to codex-rs):
just fmt
just fix -p <crate-you-touched>

# Run the relevant tests (project-specific is fastest), for example:
cargo test -p codex-tui
# If you have cargo-nextest installed, `just test` runs the test suite via nextest:
just test
# If you specifically want the full `--all-features` matrix, use:
cargo test --all-features
```

## Tracing / verbose logging

owl is written in Rust, so it honors the `RUST_LOG` environment variable to configure its logging behavior.

The TUI defaults to `RUST_LOG=codex_core=info,codex_tui=info,codex_rmcp_client=info` and log messages are written to `~/.owl/log/codex-tui.log`, so you can leave the following running in a separate terminal to monitor log messages as they are written:

```bash
tail -F ~/.owl/log/codex-tui.log
```

By comparison, the non-interactive mode (`owl exec`) defaults to `RUST_LOG=error`, but messages are printed inline, so there is no need to monitor a separate file.

See the Rust documentation on [`RUST_LOG`](https://docs.rs/env_logger/latest/env_logger/#enabling-logging) for more information on the configuration options.
