#!/usr/bin/env bash
# check.sh - verification for the owl fork (Codex CLI forked for Open WebUI).
# Every step's full output is logged, a PASS/FAIL summary comes last, and the
# script exits 1 if any step failed. Run from this folder (codex-rs); all paths
# are relative so it works the same here and on a cluster node.
set -u

# On the cluster's nodes rustup's default home (/opt/rust/rustup) is read-only and its
# preinstalled 1.92.0 lacks clippy/rustfmt, so rustup tries to modify it and fails.
# Use the node's writable rustup cache when it exists; this folder on its own is unaffected.
if [ -d /cache/rustup/toolchains ]; then
  export RUSTUP_HOME=/cache/rustup
fi

fail=0
summary=""
step() {
  local name=$1
  shift
  echo "===== $name: $*"
  "$@" 2>&1
  local rc=$?
  if [ $rc -eq 0 ]; then
    summary+="PASS  $name"$'\n'
  else
    summary+="FAIL  $name (exit $rc)"$'\n'
    fail=1
  fi
}

echo "===== environment"
uname -a
echo "cpus=$(nproc)"
free -h
df -h . | tail -1
cargo --version
rustc --version

step build             cargo build -p codex-cli -p codex-exec
step tui-lib-tests     cargo test -p codex-tui --lib
step api-tests         cargo test -p codex-api
step otel-tests        cargo test -p codex-otel
step core-lib-tests    cargo test -p codex-core --lib
# core integration tests resolve codex-linux-sandbox from target/debug at runtime
step build-sandbox     cargo build -p codex-linux-sandbox
# ...and the rmcp-client test helpers (test_stdio_server et al.); cargo test -p
# codex-core does not build another crate's binaries, so build them first.
step build-rmcp-test-bins cargo build -p codex-rmcp-client
# Owl-local: 4 threads (was 8) - each integration test runs its own tokio runtime plus sandbox
# subprocesses; 8 concurrent tests oversubscribe 8-core CI nodes and made timing-sensitive tests flake.
step core-integration  bash -c 'RUST_TEST_THREADS=4 CARGO_BUILD_JOBS=4 cargo test -p codex-core --test all'
step exec-tests        bash -c 'RUST_TEST_THREADS=4 cargo test -p codex-exec'
# app-server integration spawns the codex-app-server binary per test; cap threads
# so the machine isn't swamped (unbounded threads time out on small machines).
step app-server-tests  bash -c 'RUST_TEST_THREADS=8 cargo test -p codex-app-server'
step cli-tests         cargo test -p codex-cli
step clippy            cargo clippy -p codex-tui -p codex-core -p codex-protocol -p codex-exec -p codex-cli -p codex-api -p codex-app-server -p codex-otel --all-targets
step fmt               cargo fmt --check

printf '\n===== summary\n%s' "$summary"
exit "$fail"
