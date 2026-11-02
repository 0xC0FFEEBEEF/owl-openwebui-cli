#!/usr/bin/env bash
# End-to-end proof for install.sh in this folder. Run it from the fork folder:
#
#   bash scripts/install-selftest.sh
#
# What it proves, in order:
#   1. ./install.sh --help prints the usage and exits 0.
#   2. ./install.sh refuses an unknown option (exit 1) instead of installing something.
#   3. ./install.sh installs a working release binary into a scratch prefix - taking the
#      prebuilt binary bundled in prebuilt/ when this repo has one that runs here, and
#      compiling from source otherwise.
#   4. owl is found through PATH from that prefix and --version runs.
#   5. With an empty config folder, the first `owl` run shows the connect screen: it asks for
#      an Open WebUI server (URL, hostname or IP) and an optional API token - no ChatGPT
#      sign-in, and no upstream "Update available ... github.com/openai/codex" banner anywhere
#      - and confirms it saved them.
#   6. Those values are really written to config.toml (server + masked token).
#   7. A second `owl` run does not ask again.
#   8. The saved config alone (no environment variables) drives a real turn against the
#      stand-in Open WebUI server, which receives the saved bearer token. The scratch project
#      folder is made into a git repo first, because `owl exec` only runs inside a trusted
#      directory (a git repo) unless you pass --skip-git-repo-check - the same rule upstream.
#   9. A config folder that already holds a ChatGPT login left behind by upstream Codex
#      (auth.json plus a config.toml with no Open WebUI section) still gets the connect screen,
#      and the turn that follows talks only to the Open WebUI server with the token owl saved -
#      this is the bug report "I never got the setup screen".
#   10. Re-running install.sh upgrades in place, leaves the saved config.toml alone, and the
#      reinstalled binary still runs.
#   11. ./install.sh --force-build ignores the bundled prebuilt and compiles the release
#       binary from source. Set OWL_SELFTEST_SKIP_SOURCE=1 to skip that slow step locally;
#       the cluster certification always runs it.
#   12. ./install.sh installs the bundled prebuilt binary on a PATH that has no cargo at
#       all - proof the fast path needs no Rust toolchain (skipped when no prebuilt is bundled).
#
# Everything happens in a scratch directory under ${TMPDIR:-/tmp}: this repo, your ~/.owl
# and your ~/.local/bin are never touched. Because that scratch CODEX_HOME is under /tmp, owl
# prints one warning ("could not update PATH: ... helper binaries under temporary dir") on
# every run here; it refuses to install its helper binaries in a temp folder, which is the
# right behaviour for a scratch run. A normal install into your home folder does not print it,
# and owl still runs normally (its Linux sandbox helper is the owl binary itself), so the steps
# below check the line that matters (for example `owl 0.90.0` from --version) rather than the
# whole output - every step still has to exit 0. The stand-in Open WebUI server runs on
# 127.0.0.1 because the build nodes cannot reach a LAN server. It needs cargo and python3, and
# a few GB of free disk for the release build. It does not deliberately break any source file,
# so build-failure propagation is checked separately (install.sh prints
# "the build failed - nothing was installed." and exits 1).
set -u

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
install_sh="$repo_root/install.sh"

# Set by test_fresh_install: the binary file install.sh actually installed from
# (prebuilt/ or codex-rs/target/release/), so test_upgrade_in_place can compare against it.
installed_binary_source=""

# Build nodes keep the pinned toolchain (which codex-rs/rust-toolchain.toml asks for) under
# /cache/rustup; the image default is missing components. Same guard as codex-rs/check.sh.
if [[ -d /cache/rustup/toolchains ]]; then export RUSTUP_HOME=/cache/rustup; fi
if [[ -d /cache/cargo ]]; then export CARGO_HOME=/cache/cargo; fi

scratch=$(mktemp -d "${TMPDIR:-/tmp}/owl-install-selftest.XXXXXX") || exit 1
prefix="$scratch/prefix"
codex_home="$scratch/codex-home"
config_file="$codex_home/config.toml"
project_dir="$scratch/project"
mock_log="$scratch/mock-requests.log"
test_token="selftest-token-42"
export CODEX_HOME="$codex_home"
# owl resolves CODEX_HOME with canonicalize(), so the folder has to exist before it runs; the
# config file inside it is still absent, which is what the connect-screen step needs.
mkdir -p "$prefix" "$codex_home" "$project_dir" || exit 1
mock_pid=""
mock_port=""

cleanup() {
  if [[ -n "$mock_pid" ]] && kill -0 "$mock_pid" 2>/dev/null; then kill "$mock_pid" 2>/dev/null; fi
  rm -rf "$scratch"
}
trap cleanup EXIT

fail=0
summary=""
step() {
  local name=$1
  shift
  echo
  echo "===== $name"
  if "$@"; then
    summary+="PASS  $name"$'\n'
  else
    summary+="FAIL  $name"$'\n'
    fail=1
  fi
}

# ---------------------------------------------------------------- helpers

write_helpers() {
  cat >"$scratch/owui_mock_server.py" <<'PYEOF'
#!/usr/bin/env python3
"""A stand-in Open WebUI server: /api/v1/models plus a streaming /api/v1/chat/completions.

It exists because this machine may not be able to reach a real Open WebUI server. It logs
every request (path, Authorization header, requested model) so the self-test can assert that
the values saved by the connect screen are the values actually used.
"""
import argparse
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG_PATH = ""
REPLY_CHUNKS = ("Mock ", "WebUI ", "is thinking.", " Done.")


def log(line):
    with open(LOG_PATH, "a") as handle:
        handle.write(line + "\n")


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):  # keep the stderr log empty
        pass

    def auth(self):
        return self.headers.get("Authorization", "-")

    def do_GET(self):
        path = self.path.rstrip("/")
        log(f"GET {path} auth={self.auth()}")
        if not path.endswith("/models"):
            self.send_error(404, "not a models endpoint")
            return
        body = json.dumps(
            {
                "data": [
                    {"id": "mock-gpt-mini", "name": "Mock GPT Mini", "object": "model"},
                    {"id": "mock-second", "name": "Mock Second", "object": "model"},
                ]
            }
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        path = self.path.rstrip("/")
        length = int(self.headers.get("content-length", 0) or 0)
        try:
            payload = json.loads(self.rfile.read(length) or b"{}")
        except json.JSONDecodeError:
            payload = {}
        log(f"POST {path} auth={self.auth()} model={payload.get('model')}")
        if not path.endswith("/chat/completions"):
            self.send_error(404, "not a chat completions endpoint")
            return
        events = [{"id": "mock-1", "object": "chat.completion.chunk",
                   "choices": [{"index": 0, "delta": {"role": "assistant", "content": ""}}]}]
        for chunk in REPLY_CHUNKS:
            events.append({"id": "mock-1", "object": "chat.completion.chunk",
                           "choices": [{"index": 0, "delta": {"content": chunk}}]})
        events.append({"id": "mock-1", "object": "chat.completion.chunk",
                       "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]})
        body = "".join(f"data: {json.dumps(event)}\n\n" for event in events).encode()
        body += b"data: [DONE]\n\n"
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def main():
    global LOG_PATH
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=0, help="0 lets the OS pick a free port")
    parser.add_argument("--log", required=True)
    parser.add_argument("--port-file", required=True)
    args = parser.parse_args()
    LOG_PATH = args.log
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    with open(args.port_file, "w") as handle:
        handle.write(f"{server.server_address[1]}\n")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    sys.exit(main())
PYEOF

  cat >"$scratch/owl_connect_probe.py" <<'PYEOF'
#!/usr/bin/env python3
"""Run the Owl TUI inside a pty and check what it asks for on a fresh config folder.

  --mode connect    the first run: the connect screen must ask for a server and an API token
                    (and never a ChatGPT sign-in), the typed values must land in the form, the
                    token must be masked, and the screen must confirm it saved them.
  --mode no-prompt  a later run: the connect prompt must not appear at all.

The driver answers the terminal queries crossterm waits for (device attributes, cursor
position, kitty keyboard flags) - without those answers ratatui draws nothing and the probe
looks empty. Text and Enter are sent as separate writes with a pause in between.
"""
import argparse
import fcntl
import os
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import termios
import time

ANSI = re.compile(
    rb"\x1b\[[0-9;?!>]*[a-zA-Z]"  # CSI: colours, cursor moves, kitty queries
    rb"|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)"  # OSC, ended by BEL or ST (crossterm asks for colours)
    rb"|\x1b\([0-9A-B]|\x1b[=>M]|\x1b"  # charset selects, kitty push, reverse index, stray ESC
)
# Spaces are matched loosely on purpose: ratatui does not rewrite a space cell that already
# has the default style, so "Connect Owl to Open WebUI" arrives as "ConnectOwltoOpen WebUI".
SERVER_NEEDLE = "Server URL or hostname/IP"
TOKEN_NEEDLE = "API token (optional"
CONNECT_NEEDLE = "Connect Owl to Open WebUI"
SAVED_NEEDLE = "Saved to config.toml"
WELCOME_NEEDLE = "Welcome to Owl"
LOGIN_NEEDLES = ("Sign in with ChatGPT", "chatgpt.com")
# owl must not point at the upstream project on any screen it shows before the first turn.
UPSTREAM_NEEDLES = ("Update available", "github.com/openai/codex")


class Screen:
    def __init__(self, rows, cols):
        self.rows = rows
        self.cols = cols
        self.raw = bytearray()

    def text(self):
        return ANSI.sub(b"", bytes(self.raw)).decode("utf-8", "replace")

    def flat(self):
        """The screen text with all whitespace removed, so needles survive ratatui's cursor moves."""
        return re.sub(r"\s+", "", self.text())

    def has(self, needle):
        return re.sub(r"\s+", "", needle) in self.flat()


class OwlDied(Exception):
    pass


def answer_queries(master, chunk, screen):
    """Reply to the terminal queries crossterm sends before ratatui will render."""
    if re.search(rb"\x1b\[[0-9;]*c", chunk):
        os.write(master, b"\x1b[?1;2c")  # a plain VT100-ish terminal
    if re.search(rb"\x1b\[6n", chunk):
        os.write(master, f"\x1b[{screen.rows};1R".encode())
    if re.search(rb"\x1b\[[?>][0-9;]*u", chunk):
        os.write(master, b"\x1b[?0u")  # no kitty keyboard enhancements supported


def pump(master, screen, seconds):
    """Read output for a while, answering queries; raise OwlDied if the child is gone."""
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        readable, _, _ = select.select([master], [], [], 0.1)
        if not readable:
            continue
        try:
            chunk = os.read(master, 8192)
        except OSError:
            raise OwlDied("the owl process closed the terminal")
        if not chunk:
            raise OwlDied("the owl process closed the terminal")
        screen.raw += chunk
        answer_queries(master, chunk, screen)


def wait_for(master, screen, needle, timeout):
    deadline = time.monotonic() + timeout
    while not screen.has(needle):
        if time.monotonic() > deadline:
            return False
        pump(master, screen, 0.3)
    return True


def send(master, screen, text, enter):
    os.write(master, text.encode())
    pump(master, screen, 0.4)  # let the app drain the characters before Enter
    if enter:
        os.write(master, b"\r")
        pump(master, screen, 0.4)


def finish(master, proc, screen, code, reason):
    try:
        os.write(master, b"\x03")  # Ctrl-C, the way a person would leave the screen
    except OSError:
        pass
    pump(master, screen, 0.5)
    if proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)
    if code == 0:
        print("PROBE: ok")
    else:
        print(f"PROBE: fail - {reason}")
        tail = screen.text().splitlines()[-30:]
        print("--- last screen text ---")
        print("\n".join(tail))
    return code


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=("connect", "no-prompt"), default="connect")
    parser.add_argument("--bin", required=True, help="path to the installed owl binary")
    parser.add_argument("--cwd", required=True, help="project directory to run owl in")
    parser.add_argument("--host", default="", help="what to type into the Server field")
    parser.add_argument("--token", default="", help="what to type into the API token field")
    parser.add_argument("--transcript", required=True)
    parser.add_argument("--rows", type=int, default=45)
    parser.add_argument("--cols", type=int, default=100)
    args = parser.parse_args()

    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", args.rows, args.cols, 0, 0))
    screen = Screen(args.rows, args.cols)
    try:
        proc = subprocess.Popen(
            [args.bin],
            stdin=slave,
            stdout=slave,
            stderr=slave,
            cwd=args.cwd,
            env={**os.environ, "TERM": "xterm-256color"},
        )
    finally:
        os.close(slave)

    try:
        code, reason = run_probe(master, proc, screen, args)
    except OwlDied as err:
        code, reason = 1, str(err)
    finally:
        with open(args.transcript, "w") as handle:
            handle.write(screen.text())
    return finish(master, proc, screen, code, reason)


def run_probe(master, proc, screen, args):
    if args.mode == "no-prompt":
        if not wait_for(master, screen, WELCOME_NEEDLE, 30):
            return 1, f"never saw {WELCOME_NEEDLE!r} on a run that already has config.toml"
        pump(master, screen, 2.0)
        for needle in (SERVER_NEEDLE, TOKEN_NEEDLE, CONNECT_NEEDLE):
            if screen.has(needle):
                return 1, f"the connect prompt appeared again: {needle!r}"
        for needle in LOGIN_NEEDLES:
            if needle.lower() in screen.flat().lower():
                return 1, f"a ChatGPT login screen showed up: {needle!r}"
        for needle in UPSTREAM_NEEDLES:
            if screen.has(needle):
                return 1, f"owl pointed at upstream Codex: {needle!r}"
        print("welcome screen shown, connect prompt absent")
        return 0, ""

    if not wait_for(master, screen, SERVER_NEEDLE, 30):
        return 1, f"the first run never asked for a server ({SERVER_NEEDLE!r} not on screen)"
    if not wait_for(master, screen, TOKEN_NEEDLE, 5):
        return 1, f"the first run never asked for an API token ({TOKEN_NEEDLE!r} not on screen)"
    if not screen.has(CONNECT_NEEDLE):
        return 1, f"the screen is not the Open WebUI connect step ({CONNECT_NEEDLE!r} missing)"
    lowered = screen.flat().lower()
    for needle in LOGIN_NEEDLES:
        if needle.lower() in lowered:
            return 1, f"a ChatGPT login screen showed up: {needle!r}"
    for needle in UPSTREAM_NEEDLES:
        if screen.has(needle):
            return 1, f"owl pointed at upstream Codex: {needle!r}"
    print("connect screen shown: Open WebUI server + API token, no ChatGPT sign-in")

    if not args.host:
        return 1, "no --host given for the connect probe"
    send(master, screen, args.host, True)
    if not wait_for(master, screen, args.host, 10):
        return 1, f"the typed server {args.host!r} never appeared in the Server field"

    if args.token:
        send(master, screen, args.token, True)
        masked = "*" * len(args.token)
        if not wait_for(master, screen, masked, 5):
            return 1, "the API token was not masked on screen"
        if args.token in screen.text():
            return 1, "the API token was echoed in clear text on screen"
        print(f"server and token accepted (token shown as {masked})")
    else:
        print("server accepted, token left empty")

    if not wait_for(master, screen, SAVED_NEEDLE, 20):
        return 1, f"the connect screen never confirmed the save ({SAVED_NEEDLE!r} not shown)"
    for needle in UPSTREAM_NEEDLES:
        if screen.has(needle):
            return 1, f"owl pointed at upstream Codex while saving: {needle!r}"
    print("connect screen confirmed: saved to config.toml")
    return 0, ""


if __name__ == "__main__":
    sys.exit(main())
PYEOF
}

start_mock() {
  python3 "$scratch/owui_mock_server.py" --port 0 --log "$mock_log" \
    --port-file "$scratch/mock.port" >"$scratch/mock.stderr" 2>&1 &
  mock_pid=$!
  local _
  for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    [[ -s "$scratch/mock.port" ]] && break
    sleep 0.25
  done
  if [[ ! -s "$scratch/mock.port" ]]; then
    echo "the stand-in Open WebUI server did not start:"
    cat "$scratch/mock.stderr"
    return 1
  fi
  mock_port=$(cat "$scratch/mock.port")
  echo "stand-in Open WebUI server: http://127.0.0.1:$mock_port/api/v1 (pid $mock_pid)"
}

owl_probe() {  # owl_probe MODE [--home DIR] EXTRA_ARGS...  (DIR defaults to the main scratch home)
  local mode=$1 probe_home="$codex_home"
  shift
  if [[ ${1-} == "--home" ]]; then
    probe_home=$2
    shift 2
  fi
  env -u OPENWEBUI_BASE_URL -u OPENWEBUI_API_KEY -u OPENAI_API_KEY -u CODEX_API_KEY \
    CODEX_HOME="$probe_home" python3 "$scratch/owl_connect_probe.py" \
    --mode "$mode" --bin "$prefix/owl" --cwd "$project_dir" "$@"
}

checksum() {  # md5sum on Linux, cksum where it is missing (e.g. macOS)
  if command -v md5sum >/dev/null 2>&1; then
    md5sum "$1" | awk '{print $1}'
  else
    cksum "$1" | awk '{print $1}'
  fi
}

wait_for_in_file() {  # wait_for_in_file PATTERN FILE - the config write is not synchronous
  local pattern=$1 file=$2 _
  for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    grep -q "$pattern" "$file" && return 0
    sleep 0.5
  done
  return 1
}

# ---------------------------------------------------------------- steps

test_help() {
  bash "$install_sh" --help >"$scratch/help.out" 2>&1 || return 1
  grep -q "Usage: ./install.sh" "$scratch/help.out" || {
    echo "no usage line in --help output"
    return 1
  }
  head -3 "$scratch/help.out"
}

test_unknown_option() {
  local rc=0
  bash "$install_sh" --definitely-not-an-option >"$scratch/unknown.out" 2>&1 || rc=$?
  [[ $rc -eq 1 ]] || {
    echo "expected exit 1 for an unknown option, got $rc"
    return 1
  }
  grep -q "unknown option" "$scratch/unknown.out" || {
    echo "it did not name the unknown option:"
    tail -3 "$scratch/unknown.out"
    return 1
  }
  echo "exit 1 with: $(head -1 "$scratch/unknown.out")"
}

test_fresh_install() {
  command -v cargo >/dev/null 2>&1 || {
    echo "cargo is required for this self-test (install.sh also tells you to install rustup)"
    return 1
  }
  local rc=0
  bash "$install_sh" --prefix "$prefix" >"$scratch/install1.out" 2>&1 || rc=$?
  if [[ $rc -ne 0 ]]; then
    echo "install.sh exited $rc:"
    tail -25 "$scratch/install1.out"
    return 1
  fi
  if grep -qF "using the prebuilt binary bundled in this repo" "$scratch/install1.out"; then
    installed_binary_source="$repo_root/prebuilt/owl-x86_64-unknown-linux-gnu"
    echo "install path taken: bundled prebuilt binary"
  elif grep -q "building owl (release)" "$scratch/install1.out"; then
    installed_binary_source="$repo_root/codex-rs/target/release/owl"
    echo "install path taken: release build from source"
  else
    echo "install.sh took an unexpected install path (neither the prebuilt line nor the build line). output was:"
    tail -15 "$scratch/install1.out"
    return 1
  fi
  [[ -f "$installed_binary_source" ]] || {
    echo "the binary this install used is missing: $installed_binary_source"
    return 1
  }
  [[ -x "$prefix/owl" ]] || {
    echo "nothing executable at $prefix/owl"
    return 1
  }
  echo "$(ls -l "$prefix/owl" | awk '{print $5}') bytes installed at $prefix/owl"
}

test_force_build() {
  if [[ "${OWL_SELFTEST_SKIP_SOURCE:-0}" == 1 ]]; then
    echo "OWL_SELFTEST_SKIP_SOURCE=1 set: skipping the slow --force-build step (the cluster certification runs it)"
    return 0
  fi
  local rc=0 force_prefix="$scratch/prefix-force"
  bash "$install_sh" --prefix "$force_prefix" --force-build >"$scratch/install_force.out" 2>&1 || rc=$?
  if [[ $rc -ne 0 ]]; then
    echo "install.sh --force-build exited $rc:"
    tail -25 "$scratch/install_force.out"
    return 1
  fi
  grep -q "building owl (release)" "$scratch/install_force.out" || {
    echo "install.sh --force-build did not compile from source. output was:"
    tail -15 "$scratch/install_force.out"
    return 1
  }
  [[ -x "$force_prefix/owl" ]] || {
    echo "nothing executable at $force_prefix/owl after --force-build"
    return 1
  }
  "$force_prefix/owl" --version >/dev/null || {
    echo "the --force-build binary did not run"
    return 1
  }
  echo "--force-build compiled from source: $force_prefix/owl ($("$force_prefix/owl" --version))"
}

test_prebuilt_without_toolchain() {
  if [[ ! -f "$repo_root/prebuilt/owl-x86_64-unknown-linux-gnu"
        || "$(uname -s)/$(uname -m)" != "Linux/x86_64" ]]; then
    echo "no bundled prebuilt binary in this tree (or not x86_64 Linux): skipping this step"
    return 0
  fi
  local rc=0 notc_prefix="$scratch/prefix-notoolchain"
  # Strip ~/.cargo/bin (and any rustup shims) from PATH: cargo must be invisible here.
  local bare_path="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
  env PATH="$bare_path" bash "$install_sh" --prefix "$notc_prefix" >"$scratch/install_notc.out" 2>&1 || rc=$?
  if [[ $rc -ne 0 ]]; then
    echo "install.sh failed with no cargo on PATH (exit $rc) - the prebuilt path should not need a toolchain:"
    tail -25 "$scratch/install_notc.out"
    return 1
  fi
  grep -qF "using the prebuilt binary bundled in this repo" "$scratch/install_notc.out" || {
    echo "install.sh did not take the prebuilt path without a toolchain. output was:"
    tail -15 "$scratch/install_notc.out"
    return 1
  }
  [[ -x "$notc_prefix/owl" ]] || {
    echo "nothing executable at $notc_prefix/owl"
    return 1
  }
  echo "toolchain-free install works: $notc_prefix/owl ($("$notc_prefix/owl" --version 2>&1 | head -1))"
}

test_on_path_and_version() {
  PATH="$prefix:$PATH"
  local found version
  found=$(command -v owl) || {
    echo "owl was not found on PATH after installing into $prefix"
    return 1
  }
  [[ "$found" == "$prefix/owl" ]] || {
    echo "owl resolved to $found instead of $prefix/owl"
    return 1
  }
  version=$("$found" --version 2>&1) || {
    echo "owl --version failed: $version"
    return 1
  }
  # The version line is what matters; owl may also print the scratch-home PATH warning
  # described at the top of this file.
  local version_line
  version_line=$(grep -E '^owl [0-9]+\.[0-9]+\.[0-9]+' <<<"$version" | head -1)
  [[ -n "$version_line" ]] || {
    echo "owl --version printed no version line. output was:"
    sed 's/^/  /' <<<"$version"
    return 1
  }
  echo "PATH lookup: $found"
  echo "owl --version: $version_line"
  if grep -q "could not update PATH" <<<"$version"; then
    echo "  (plus the scratch-home PATH warning, expected only because CODEX_HOME is under /tmp here)"
  fi
}

test_connect_screen() {
  start_mock || return 1
  local rc=0
  owl_probe connect --host "127.0.0.1:$mock_port" --token "$test_token" \
    --transcript "$scratch/connect-screen.txt" >"$scratch/probe1.out" 2>&1 || rc=$?
  cat "$scratch/probe1.out"
  if [[ $rc -ne 0 ]]; then
    echo "the connect probe failed (exit $rc)"
    return 1
  fi
}

test_values_saved_to_config() {
  if [[ ! -f "$config_file" ]]; then
    echo "owl never created $config_file"
    return 1
  fi
  local pattern
  for pattern in '^\[model_providers\.openwebui\]' 'name = "Open WebUI"' \
    "base_url = \"http://127.0.0.1:$mock_port/api/v1\"" "experimental_bearer_token = \"$test_token\""; do
    if ! wait_for_in_file "$pattern" "$config_file"; then
      echo "config.toml is missing: $pattern"
      cat "$config_file"
      return 1
    fi
  done
  echo "saved in $config_file:"
  grep -v '^$' "$config_file" | sed 's/^/  /'
  if ! grep -q "wire_api" "$config_file"; then
    echo "  (no wire_api line: it comes from the built-in openwebui provider)"
  fi
}

test_second_run_does_not_ask() {
  local rc=0
  owl_probe no-prompt --transcript "$scratch/second-run.txt" >"$scratch/probe2.out" 2>&1 || rc=$?
  cat "$scratch/probe2.out"
  if [[ $rc -ne 0 ]]; then
    echo "the second run still asked for the server or token (exit $rc)"
    return 1
  fi
}

test_saved_config_drives_a_turn() {
  local requests_before=0 rc=0
  [[ -f "$mock_log" ]] && requests_before=$(wc -l <"$mock_log")
  # `owl exec` only runs inside a trusted directory (a git repo) unless you pass
  # --skip-git-repo-check, which is upstream behaviour, not a fork quirk. A real user runs it
  # in their own project, so make the scratch project folder a git repo and run without the flag.
  local in_git_repo="no"
  if command -v git >/dev/null 2>&1; then
    git init -q "$project_dir" || {
      echo "git init failed in $project_dir"
      return 1
    }
    in_git_repo="yes"
  fi
  if [[ "$in_git_repo" == "yes" ]]; then
    echo "turn runs in $project_dir, made a git repo so owl accepts it as a trusted directory"
    (cd "$project_dir" && env -u OPENWEBUI_BASE_URL -u OPENWEBUI_API_KEY -u OPENAI_API_KEY \
      -u CODEX_API_KEY CODEX_HOME="$codex_home" "$prefix/owl" exec "say hi") \
      >"$scratch/exec.out" 2>&1 || rc=$?
  else
    echo "no git on this machine, so the turn uses the documented owl exec --skip-git-repo-check"
    (cd "$project_dir" && env -u OPENWEBUI_BASE_URL -u OPENWEBUI_API_KEY -u OPENAI_API_KEY \
      -u CODEX_API_KEY CODEX_HOME="$codex_home" "$prefix/owl" exec --skip-git-repo-check "say hi") \
      >"$scratch/exec.out" 2>&1 || rc=$?
  fi
  if [[ $rc -ne 0 ]]; then
    echo "owl exec exited $rc:"
    tail -25 "$scratch/exec.out"
    return 1
  fi
  local needle
  for needle in "model: mock-gpt-mini" "provider: openwebui" "Mock WebUI is thinking. Done."; do
    grep -q "$needle" "$scratch/exec.out" || {
      echo "the turn output does not contain: $needle"
      head -15 "$scratch/exec.out"
      return 1
    }
  done
  local new_requests
  new_requests=$(tail -n +"$((requests_before + 1))" "$mock_log")
  grep -q "Bearer $test_token" <<<"$new_requests" || {
    echo "the saved API token was not sent to the server. requests since this turn:"
    echo "$new_requests"
    return 1
  }
  grep -q "model=mock-gpt-mini" <<<"$new_requests" || {
    echo "the server was asked for a model that is not its own:"
    echo "$new_requests"
    return 1
  }
  echo "turn used only config.toml; server saw:"
  sed 's/^/  /' <<<"$new_requests"
}

test_leftover_chatgpt_login_is_ignored() {
  # The bug this guards: a machine that once ran upstream Codex has a ChatGPT
  # auth.json (and a config.toml with no Open WebUI section) in its codex home.
  # owl must still show the connect screen and then talk only to Open WebUI,
  # instead of quietly picking the saved OpenAI/ChatGPT credentials.
  local other_home="$scratch/other-codex-home"
  local other_config="$other_home/config.toml"
  local other_token="selftest-leftover-77"
  local rc=0
  mkdir -p "$other_home" || return 1
  # A ChatGPT "codex login" auth.json, with tokens that are obviously fake. The
  # id token is a real base64url JWT so the file loads the way a saved login does.
  cat >"$other_home/auth.json" <<'JSON'
{
  "OPENAI_API_KEY": null,
  "tokens": {
    "id_token": "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJlbWFpbCI6InNvbWVvbmVAZXhhbXBsZS5jb20iLCJlbWFpbF92ZXJpZmllZCI6dHJ1ZSwiaHR0cHM6Ly9hcGkub3BlbmFpLmNvbS9hdXRoIjp7ImNoYXRncHRfcGxhbl90eXBlIjoicGx1cyIsImNoYXRncHRfdXNlcl9pZCI6InVzZXItMTIzNDUiLCJ1c2VyX2lkIjoidXNlci0xMjM0NSJ9fQ.c2lnbmF0dXJl",
    "access_token": "fake-access-token",
    "refresh_token": "fake-refresh-token",
    "account_id": "fake-account"
  }
}
JSON
  cat >"$other_config" <<'TOML'
# A config.toml of the kind upstream Codex writes: reasoning effort, no provider.
model_reasoning_effort = "high"
TOML

  # 1. the connect screen still appears in that home.
  local probe_out="$scratch/probe-leftover.out" rc_probe=0
  owl_probe connect --home "$other_home" --host "127.0.0.1:$mock_port" \
    --token "$other_token" --transcript "$scratch/probe-leftover.txt" \
    >"$probe_out" 2>&1 || rc_probe=$?
  cat "$probe_out"
  if [[ $rc_probe -ne 0 ]]; then
    echo "the connect probe failed in a home that already had a ChatGPT login (exit $rc_probe)"
    return 1
  fi
  if ! grep -q "experimental_bearer_token = \"$other_token\"" "$other_config"; then
    echo "the connect screen did not save its token in $other_config"
    cat "$other_config"
    return 1
  fi

  # 2. a turn in that home goes to Open WebUI with the token owl just saved, not to OpenAI.
  local requests_before=0
  [[ -f "$mock_log" ]] && requests_before=$(wc -l <"$mock_log")
  local out="$scratch/exec-leftover.out"
  if command -v git >/dev/null 2>&1 && [[ -d "$project_dir/.git" ]]; then
    (cd "$project_dir" && env -u OPENWEBUI_BASE_URL -u OPENWEBUI_API_KEY -u OPENAI_API_KEY \
      -u CODEX_API_KEY CODEX_HOME="$other_home" "$prefix/owl" exec "say hi") \
      >"$out" 2>&1 || rc=$?
  else
    (cd "$project_dir" && env -u OPENWEBUI_BASE_URL -u OPENWEBUI_API_KEY -u OPENAI_API_KEY \
      -u CODEX_API_KEY CODEX_HOME="$other_home" "$prefix/owl" exec --skip-git-repo-check "say hi") \
      >"$out" 2>&1 || rc=$?
  fi
  if [[ $rc -ne 0 ]]; then
    echo "owl exec exited $rc in the home with the leftover ChatGPT login:"
    tail -25 "$out"
    return 1
  fi
  local needle
  for needle in "model: mock-gpt-mini" "provider: openwebui"; do
    grep -q "$needle" "$out" || {
      echo "the turn in that home does not report: $needle"
      head -15 "$out"
      return 1
    }
  done
  local new_requests
  new_requests=$(tail -n +"$((requests_before + 1))" "$mock_log")
  grep -q "Bearer $other_token" <<<"$new_requests" || {
    echo "the turn did not use the Open WebUI token owl saved. requests since this turn:"
    echo "$new_requests"
    return 1
  }
  echo "auth.json still in $other_home: $(grep -c fake-access-token "$other_home/auth.json") fake token, unused"
  echo "the turn used the openwebui provider; server saw:"
  sed 's/^/  /' <<<"$new_requests"
}

test_upgrade_in_place() {
  local md5_before
  md5_before=$(checksum "$config_file")
  local rc=0
  env CODEX_HOME="$codex_home" bash "$install_sh" --prefix "$prefix" \
    --base-url "http://127.0.0.1:$mock_port/api/v1" >"$scratch/install2.out" 2>&1 || rc=$?
  if [[ $rc -ne 0 ]]; then
    echo "the second install.sh run exited $rc:"
    tail -25 "$scratch/install2.out"
    return 1
  fi
  grep -q "left $config_file alone" "$scratch/install2.out" || {
    echo "install.sh did not report leaving the saved provider section alone:"
    grep "config.toml" "$scratch/install2.out" || tail -5 "$scratch/install2.out"
    return 1
  }
  local md5_after
  md5_after=$(checksum "$config_file")
  [[ "$md5_before" == "$md5_after" ]] || {
    echo "the upgrade changed config.toml ($md5_before -> $md5_after)"
    return 1
  }
  [[ -x "$prefix/owl" ]] || {
    echo "$prefix/owl is gone after the upgrade"
    return 1
  }
  local installed built
  installed=$(checksum "$prefix/owl" | awk '{print $1}')
  built=$(checksum "$installed_binary_source" | awk '{print $1}')
  [[ "$installed" == "$built" ]] || {
    echo "the reinstalled binary differs from the binary the fresh install used ($installed_binary_source)"
    return 1
  }
  PATH="$prefix:$PATH" owl --version || return 1
  echo "config.toml untouched ($md5_after), binary reinstalled from the same source ($installed_binary_source)"
}

# ---------------------------------------------------------------- run

echo "owl install.sh self-test"
uname -a
echo "  cpus:    $(nproc)"
echo "  memory:  $(free -h | awk '/^Mem:/{print $2 " total, " $7 " available"}')"
echo "  repo:    $repo_root"
echo "  scratch: $scratch"
echo "  cargo:   $(cargo --version 2>&1 || echo 'cargo not found')"
echo "  rustc:   $(rustc --version 2>&1 || echo 'rustc not found')"
echo "  python:  $(python3 --version 2>&1 || echo 'python3 not found')"
echo "  git:     $(git --version 2>&1 || echo 'git not found (the exec turn then uses --skip-git-repo-check)')"
echo "  disk:    $(df -h "${TMPDIR:-/tmp}" | tail -1 | awk '{print $4 " free in " $NF}')"

write_helpers || exit 1

step "install.sh --help" test_help
step "install.sh refuses an unknown option" test_unknown_option
step "fresh install reaches a working owl (prebuilt fast path when bundled)" test_fresh_install
step "owl is on PATH and reports its version" test_on_path_and_version
step "first run asks for the Open WebUI server and token" test_connect_screen
step "both values are saved to config.toml" test_values_saved_to_config
step "second run does not ask again" test_second_run_does_not_ask
step "saved config alone drives a turn and sends the token" test_saved_config_drives_a_turn
step "a leftover ChatGPT login does not hide the connect screen" test_leftover_chatgpt_login_is_ignored
step "re-running install.sh upgrades in place" test_upgrade_in_place
step "install.sh --force-build compiles from source" test_force_build
step "prebuilt install works with no Rust toolchain" test_prebuilt_without_toolchain

printf '\n===== summary\n%s' "$summary"
if [[ $fail -eq 0 ]]; then
  printf '\nINSTALL SELFTEST: PASS\n'
else
  printf '\nINSTALL SELFTEST: FAIL\n'
fi
exit $fail
