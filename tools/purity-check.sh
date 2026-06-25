#!/bin/sh
# Proves the `engine` crate stays client-portable: it must compile to wasm32 with no server-only
# deps (sqlx / tokio / clock / fs). If this breaks, something impure leaked into the pure core —
# move it up to the `api` crate. Keeping the engine wasm-clean is what lets the SAME transition
# rules run client-side in a browser demo later.
set -e
cd "$(dirname "$0")/.."

# cargo may live under ~/.cargo in non-interactive shells.
if ! command -v cargo >/dev/null 2>&1; then
  [ -s "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
fi

rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true

echo "== cargo check --target wasm32-unknown-unknown -p engine"
cargo check --target wasm32-unknown-unknown -p engine
echo "purity-check: engine is wasm32-clean."
