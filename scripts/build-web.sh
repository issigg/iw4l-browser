#!/usr/bin/env bash
# Build the browser (wasm32 + WebGPU) version into crates/web/www/pkg.
# Needs: rustup with the wasm32-unknown-unknown target, and wasm-bindgen-cli 0.2.129
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.129
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

want=0.2.129
have=$(wasm-bindgen --version 2>/dev/null | awk '{print $2}' || true)
if [ "$have" != "$want" ]; then
  echo "wasm-bindgen $want required (found: ${have:-none})" >&2
  echo "  cargo install wasm-bindgen-cli --version $want" >&2
  exit 1
fi

cargo build -p web --target wasm32-unknown-unknown --profile play
wasm-bindgen --target web --out-dir crates/web/www/pkg \
  target/wasm32-unknown-unknown/play/web.wasm
echo "built crates/web/www/pkg; serve with:"
echo "  node crates/web/www/serve.mjs <your MW2 folder> 8137"
