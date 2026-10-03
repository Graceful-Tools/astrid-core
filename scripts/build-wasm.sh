#!/usr/bin/env bash
# Build astrid-rules' stateless door as WebAssembly, for astrid-web.
#
#   scripts/build-wasm.sh <out-dir>
#
# Writes two wasm-bindgen packages from one build:
#   <out-dir>/node/   CommonJS for Node (Next.js route handlers and server components; vitest)
#   <out-dir>/web/    an ES module for the browser, initialised with the .wasm's URL
#
# Needs the wasm32-unknown-unknown target and the wasm-bindgen CLI at the exact version
# Cargo.lock pins (the CLI refuses a module from any other). No wasm-opt pass: measured on
# 2026-10-03 it saved 5% raw and cost 8% gzipped, and leaving it out keeps the output the same
# on every machine, which a pinned revision relies on.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:?usage: scripts/build-wasm.sh <out-dir>}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"

want="$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/"/, "", $3); print $3; exit }' "$root/Cargo.lock")"
have="$(wasm-bindgen --version 2>/dev/null | awk '{ print $2 }')" || true
if [[ "$have" != "$want" ]]; then
  echo "wasm-bindgen CLI $want is needed (found: ${have:-none}):" >&2
  echo "  cargo install wasm-bindgen-cli --version $want --locked" >&2
  exit 1
fi
rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true

cd "$root"
cargo build -p astrid-rules-wasm --target wasm32-unknown-unknown --profile wasm
wasm="${CARGO_TARGET_DIR:-$root/target}/wasm32-unknown-unknown/wasm/astrid_rules_wasm.wasm"

rm -rf "$out/node" "$out/web"
wasm-bindgen "$wasm" --out-dir "$out/node" --target nodejs
wasm-bindgen "$wasm" --out-dir "$out/web" --target web

for f in "$out/node/astrid_rules_wasm_bg.wasm" "$out/web/astrid_rules_wasm_bg.wasm"; do
  printf '%s  %s bytes raw, %s gzip\n' "${f#"$out"/}" "$(wc -c <"$f" | tr -d ' ')" "$(gzip -9c "$f" | wc -c | tr -d ' ')"
done
