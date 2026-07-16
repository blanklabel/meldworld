#!/usr/bin/env bash
# Build the wasm client to dist/ (static files the server can serve directly).
# Wrapper so the wasm build uses rustup's toolchain (which has wasm32 std), not
# a Homebrew rustc that lacks it.
set -e
export PATH="$HOME/.cargo/bin:$PATH"
cd "$(dirname "$0")/../crates/meld-client"
exec trunk build "$@"
