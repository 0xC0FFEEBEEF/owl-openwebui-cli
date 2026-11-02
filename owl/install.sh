#!/usr/bin/env bash
# Install the Owl CLI: put the `owl` binary on your PATH - using the prebuilt binary bundled
# in this folder when it runs on your machine, otherwise building it from source here.
# Run ./install.sh --help for the options.
set -euo pipefail

usage() {
  cat <<'USAGE'
Install the Owl CLI: puts the `owl` binary into your PATH. When this folder contains a
prebuilt binary that runs on your machine (prebuilt/, x86_64 Linux) it is used right away -
no compiling, no Rust toolchain needed. Otherwise owl is built from this folder's source,
which needs a Rust toolchain (rustup) on the machine you install onto. Linux and macOS are
supported; on Windows build it under WSL2 (see docs/install.md for system requirements).

Usage: ./install.sh [options]
  --prefix DIR     where to install (default: $HOME/.local/bin)
  --base-url URL   write this Open WebUI base URL into config.toml (optional)
  --debug          install the debug build instead of the release build
  --skip-build     install the binary that is already in codex-rs/target/
  --force-build    ignore any bundled prebuilt binary and build from source
  --help           show this help
USAGE
}

root_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
workspace_dir="$root_dir/codex-rs"

prefix="${HOME}/.local/bin"
profile="release"
skip_build=false
force_build=false
base_url=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix)
      [[ $# -ge 2 ]] || { echo "install.sh: --prefix needs a directory" >&2; exit 1; }
      prefix="$2"
      shift 2
      ;;
    --base-url)
      [[ $# -ge 2 ]] || { echo "install.sh: --base-url needs a URL" >&2; exit 1; }
      base_url="$2"
      shift 2
      ;;
    --debug)
      profile="debug"
      shift
      ;;
    --skip-build)
      skip_build=true
      shift
      ;;
    --force-build)
      force_build=true
      shift
      ;;
    --help | -h)
      usage
      exit 0
      ;;
    *)
      echo "install.sh: unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ ! -f "$workspace_dir/Cargo.toml" ]]; then
  echo "install.sh: no Cargo.toml under $workspace_dir - run this from the fork folder." >&2
  exit 1
fi

# A prebuilt release binary may ship inside this repo (prebuilt/). Use it when it matches
# this machine and actually runs; otherwise fall back to building from source. --force-build
# and --debug never take the prebuilt.
selected_binary=""
if [[ "$skip_build" == false && "$force_build" == false && "$profile" == release
      && "$(uname -s)/$(uname -m)" == "Linux/x86_64" ]]; then
  prebuilt_candidate="$root_dir/prebuilt/owl-x86_64-unknown-linux-gnu"
  if [[ -f "$prebuilt_candidate" ]]; then
    chmod 0755 "$prebuilt_candidate" 2>/dev/null || true
    prebuilt_ok=true
    if [[ -f "$root_dir/prebuilt/SHA256SUMS" ]] && command -v sha256sum >/dev/null; then
      expected=$(awk '$2 == "owl-x86_64-unknown-linux-gnu" { print $1 }' "$root_dir/prebuilt/SHA256SUMS")
      actual=$(sha256sum "$prebuilt_candidate" | awk '{ print $1 }')
      if [[ -n "$expected" && "$expected" != "$actual" ]]; then
        echo "note: the bundled prebuilt binary does not match prebuilt/SHA256SUMS - building from source instead" >&2
        prebuilt_ok=false
      fi
    fi
    if [[ "$prebuilt_ok" == true ]]; then
      smoke_home=$(mktemp -d)
      if OWL_HOME="$smoke_home" "$prebuilt_candidate" --version >/dev/null 2>&1; then
        selected_binary="$prebuilt_candidate"
        echo "==> using the prebuilt binary bundled in this repo (prebuilt/) - no build needed"
      else
        echo "note: the bundled prebuilt binary did not run on this machine - building from source instead" >&2
      fi
      rm -rf "$smoke_home"
    fi
  fi
fi

if [[ -z "$selected_binary" && "$skip_build" == false ]]; then
  if ! command -v cargo >/dev/null 2>&1; then
    echo "install.sh: cargo was not found. Install Rust first:" >&2
    echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && source \"\$HOME/.cargo/env\"" >&2
    echo "  (or run ./install.sh --skip-build if codex-rs/target already has the binary)" >&2
    exit 1
  fi
  echo "==> building owl ($profile) in $workspace_dir"
  build_args=(build -p codex-cli --bin owl)
  if [[ "$profile" == release ]]; then
    build_args+=(--release)
  fi
  if ! (cd "$workspace_dir" && cargo "${build_args[@]}"); then
    echo "install.sh: the build failed - nothing was installed." >&2
    exit 1
  fi
elif [[ -z "$selected_binary" ]]; then
  echo "==> skipping the build, using the existing $profile binary"
fi

if [[ -n "$selected_binary" ]]; then
  binary="$selected_binary"
else
  binary="$workspace_dir/target/$profile/owl"
fi
if [[ ! -x "$binary" ]]; then
  echo "install.sh: $binary is missing or not executable." >&2
  echo "  Run ./install.sh without --skip-build, or build it first:" >&2
  echo "  (cd codex-rs && cargo build -p codex-cli --bin owl --$profile)" >&2
  exit 1
fi

echo "==> installing to $prefix"
if ! mkdir -p "$prefix"; then
  echo "install.sh: could not create $prefix" >&2
  exit 1
fi
if ! install -m 0755 "$binary" "$prefix/owl"; then
  echo "install.sh: could not copy the binary to $prefix/owl" >&2
  exit 1
fi

if [[ -n "$base_url" ]]; then
  owl_home="${OWL_HOME:-${CODEX_HOME:-$HOME/.owl}}"
  config_file="$owl_home/config.toml"
  mkdir -p "$owl_home"
  if [[ -f "$config_file" ]] && grep -q '^\[model_providers\.openwebui\]' "$config_file"; then
    echo "==> left $config_file alone: it already has a [model_providers.openwebui] section"
  else
    printf '\n[model_providers.openwebui]\nname = "Open WebUI"\nbase_url = "%s"\nwire_api = "chat"\n' \
      "$base_url" >>"$config_file" || {
      echo "install.sh: could not write $config_file" >&2
      exit 1
    }
    echo "==> wrote base_url to $config_file"
  fi
fi

if ! owl_version=$("$prefix/owl" --version); then
  echo "install.sh: the installed binary did not run." >&2
  exit 1
fi

case ":$PATH:" in
  *":$prefix:"*) ;;
  *) echo "note: $prefix is not on your PATH. Add it with:"
     echo "  export PATH=\"$prefix:\$PATH\"" ;;
esac

echo "installed: $prefix/owl ($owl_version)"
if [[ -z "$base_url" ]]; then
  echo "Next: run owl - it opens the connect screen and saves your Open WebUI server for you."
  echo "Or set OPENWEBUI_BASE_URL (and optionally OPENWEBUI_API_KEY) before starting."
fi
