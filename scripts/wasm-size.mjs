#!/usr/bin/env node
// The web binding's size, and the budget CI holds it to (task 4d05306d).
//
//   node scripts/wasm-size.mjs <file.wasm> [--budget <brotli bytes>] [--attribute]
//
// WHY NODE AND NOT A SHELL ONE-LINER. The acceptance for this crate's size is written in brotli
// bytes, and `brotli` is not on every machine this repo builds on — but Node's zlib has carried
// `brotliCompressSync` since v11, and Node is already required to build astrid-web. So the
// measuring instrument needs no install, which is the only reason the number in README.md was
// ever measured rather than estimated.
//
// WHY THE BUDGET IS ON THE PRE-`wasm-bindgen` ARTIFACT. CI's `wasm` job builds
// `astrid-rules-wasm` in the `wasm` profile and stops there; running `wasm-bindgen` would mean
// pinning and installing its CLI in CI for a number that tracks this one anyway. See README.md
// for the measured ratio between the two.
//
// `--attribute` additionally reports the code section by crate, which is what `twiggy top`
// reports. It needs a build with `--config 'profile.wasm.strip=false'`, since the name section is
// stripped from the shipped artifact; with a stripped file it says so rather than reporting zeros.

import { readFileSync } from 'node:fs'
import { gzipSync, brotliCompressSync, constants } from 'node:zlib'

const args = process.argv.slice(2)
const file = args.find((a) => !a.startsWith('--'))
if (!file) {
  console.error('usage: node scripts/wasm-size.mjs <file.wasm> [--budget <bytes>] [--attribute]')
  process.exit(2)
}
const budgetAt = args.indexOf('--budget')
const budget = budgetAt === -1 ? null : Number(args[budgetAt + 1])

const buf = readFileSync(file)
const gzip = gzipSync(buf, { level: 9 }).length
const brotli = brotliCompressSync(buf, {
  params: { [constants.BROTLI_PARAM_QUALITY]: 11 },
}).length

const kb = (n) => `${(n / 1024).toFixed(1)} KB`
console.log(`${file}`)
console.log(`  raw    ${String(buf.length).padStart(9)}  ${kb(buf.length)}`)
console.log(`  gzip   ${String(gzip).padStart(9)}  ${kb(gzip)}`)
console.log(`  brotli ${String(brotli).padStart(9)}  ${kb(brotli)}`)

if (args.includes('--attribute')) attribute(buf)

if (budget !== null) {
  if (brotli > budget) {
    console.error(
      `\nOVER BUDGET: ${brotli} brotli bytes, budget ${budget} (${kb(brotli - budget)} over).\n` +
        'The web loads this in a browser. If the growth is wanted, raise the budget HERE and in\n' +
        "README.md in the same commit, with what it bought — not quietly, and not by rounding up.\n" +
        'Run with --attribute on an unstripped build to see which crate grew:\n' +
        "  cargo build -p astrid-rules-wasm --target wasm32-unknown-unknown --profile wasm \\\n" +
        "    --config 'profile.wasm.strip=false'",
    )
    process.exit(1)
  }
  console.log(`\nwithin budget: ${brotli} <= ${budget} brotli bytes (${kb(budget - brotli)} spare)`)
}

/** The code section summed per defining crate, from the name section. `twiggy top`, without twiggy. */
function attribute(wasm) {
  let p = 8
  const leb = () => {
    let result = 0
    let shift = 0
    let byte
    do {
      byte = wasm[p++]
      result |= (byte & 0x7f) << shift
      shift += 7
    } while (byte & 0x80)
    return result >>> 0
  }

  const sections = []
  while (p < wasm.length) {
    const id = wasm[p++]
    const size = leb()
    sections.push({ id, start: p, size })
    p += size
  }

  const code = sections.find((s) => s.id === 10)
  const bodies = []
  if (code) {
    p = code.start
    const count = leb()
    for (let i = 0; i < count; i++) {
      const size = leb()
      bodies.push(size)
      p += size
    }
  }

  // Function names live in the "name" custom section, subsection 1.
  const names = new Map()
  for (const s of sections.filter((x) => x.id === 0)) {
    p = s.start
    const nameLen = leb()
    const sectionName = wasm.subarray(p, p + nameLen).toString('utf8')
    p += nameLen
    if (sectionName !== 'name') continue
    const end = s.start + s.size
    while (p < end) {
      const sub = wasm[p++]
      const subSize = leb()
      const subEnd = p + subSize
      if (sub === 1) {
        const n = leb()
        for (let i = 0; i < n; i++) {
          const idx = leb()
          const len = leb()
          names.set(idx, wasm.subarray(p, p + len).toString('utf8'))
          p += len
        }
      }
      p = subEnd
    }
  }
  if (names.size === 0) {
    console.log(
      '\n-- no name section: this build was stripped, so there is nothing to attribute.\n' +
        "   Rebuild with --config 'profile.wasm.strip=false'.",
    )
    return
  }

  // Imported functions occupy the low indices, before the defined ones.
  let imported = 0
  const imports = sections.find((s) => s.id === 2)
  if (imports) {
    p = imports.start
    const n = leb()
    for (let i = 0; i < n; i++) {
      for (let j = 0; j < 2; j++) p += leb()
      const kind = wasm[p++]
      if (kind === 0) {
        leb()
        imported++
      } else if (kind === 1) {
        p++
        const limited = wasm[p++]
        leb()
        if (limited) leb()
      } else if (kind === 2) {
        const limited = wasm[p++]
        leb()
        if (limited) leb()
      } else {
        p += 2
      }
    }
  }

  // Rust v0 mangling writes each crate as `Cs<disambiguator>_<len><name>`. The FIRST one in a
  // symbol names the crate that DEFINED the function -- so a monomorphised generic is charged to
  // whoever wrote the generic, not to whoever instantiated it. That is the attribution twiggy
  // makes, and it is the one that answers "which dependency do we carry".
  const crateOf = (name) => {
    if (!name) return '(unnamed)'
    const v0 = name.match(/Cs[0-9A-Za-z]+_(\d+)([A-Za-z0-9_]+)/)
    if (v0) return v0[2].slice(0, Number(v0[1]))
    const legacy = name.match(/^_ZN\d+([A-Za-z0-9_]+)/)
    if (legacy) return legacy[1]
    if (/^__|^rust_|^mem(cpy|set|cmp|move)|^malloc|^free|^dlmalloc/.test(name)) return '(runtime)'
    return '(other)'
  }

  const byCrate = new Map()
  const functions = []
  for (let i = 0; i < bodies.length; i++) {
    const name = names.get(imported + i)
    byCrate.set(crateOf(name), (byCrate.get(crateOf(name)) ?? 0) + bodies[i])
    functions.push([bodies[i], name ?? `#${imported + i}`])
  }
  const total = bodies.reduce((a, b) => a + b, 0)
  const data = sections.find((s) => s.id === 11)

  console.log(`\n  code  ${String(total).padStart(9)}  ${kb(total)} in ${bodies.length} functions`)
  if (data) console.log(`  data  ${String(data.size).padStart(9)}  ${kb(data.size)}`)
  console.log('\n-- code by crate --')
  for (const [crate, bytes] of [...byCrate].sort((a, b) => b[1] - a[1]).slice(0, 15)) {
    const share = ((bytes / total) * 100).toFixed(0)
    console.log(`${String(bytes).padStart(9)}  ${String(share).padStart(3)}%  ${crate}`)
  }
  console.log('\n-- biggest functions --')
  for (const [bytes, name] of functions.sort((a, b) => b[0] - a[0]).slice(0, 10)) {
    console.log(`${String(bytes).padStart(9)}  ${name.slice(0, 100)}`)
  }
}
