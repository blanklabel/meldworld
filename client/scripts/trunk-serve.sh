#!/usr/bin/env bash
# Wrapper so the wasm build uses rustup's toolchain (which has wasm32 std),
# not a Homebrew rustc that lacks it. Runs `trunk serve` in the client crate.
set -e
export PATH="$HOME/.cargo/bin:$PATH"
cd "$(dirname "$0")/../crates/meld-client"
exec trunk serve "$@"
