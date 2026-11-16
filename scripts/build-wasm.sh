#!/usr/bin/env bash
# Build astrid-rules' stateless door as WebAssembly, for astrid-web.
#
#   scripts/build-wasm.sh <out-dir>
#
# Writes two wasm-bindgen packages from two builds:
#   <out-dir>/node/   CommonJS for Node (Next.js route handlers and server components; vitest).
#                     Built with `--features markdown,orchestration`: astrid-web's server renders
#                     through it, and asks it the orchestration policy (AWTD-1685).
#   <out-dir>/web/    an ES module for the browser, initialised with the .wasm's URL. The default
#                     build, with no markdown renderer (task 4d05306d / AWTD2-162): the browser has
#                     its own, and this is the artifact CI's size budget measures.
#
# Then it asks each package for `renderMarkdown` and `orchestrationMay` and fails unless node
# answers and web refuses.
# One artifact used to feed both packages, which is how the Node package once lost the renderer.
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
wasm="${CARGO_TARGET_DIR:-$root/target}/wasm32-unknown-unknown/wasm/astrid_rules_wasm.wasm"
rm -rf "$out/node" "$out/web"

# Both builds write the same artifact path, so each is bound before the next build replaces it.
cargo build -p astrid-rules-wasm --target wasm32-unknown-unknown --profile wasm --features markdown,orchestration
wasm-bindgen "$wasm" --out-dir "$out/node" --target nodejs
cargo build -p astrid-rules-wasm --target wasm32-unknown-unknown --profile wasm
wasm-bindgen "$wasm" --out-dir "$out/web" --target web

# What each package answers, read from the packages themselves rather than from the features asked
# for: node must render markdown (astrid-web's server and its markdown parity test call it), and
# web must not carry the renderer.
node "$root/scripts/check-wasm-packages.mjs" "$out"

for f in "$out/node/astrid_rules_wasm_bg.wasm" "$out/web/astrid_rules_wasm_bg.wasm"; do
  printf '%s  %s bytes raw, %s gzip\n' "${f#"$out"/}" "$(wc -c <"$f" | tr -d ' ')" "$(gzip -9c "$f" | wc -c | tr -d ' ')"
done
