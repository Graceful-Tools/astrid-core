#!/usr/bin/env node
// Check what the two packages scripts/build-wasm.sh writes actually answer (AWTD2-162).
//
//   node scripts/check-wasm-packages.mjs <out-dir>
//
// node/ must answer `renderMarkdown`: astrid-web's server renders through it, and its
// tests/lib/core-rules-markdown-parity.test.ts holds web's renderer to the core's.
// web/ must refuse it: the browser build carries no markdown renderer (task 4d05306d).
// The same for `orchestrationMay` (AWTD-1685): the server asks the orchestration policy, and the
// browser build leaves it out.
// Both must answer an ordinary rule, so a package that refuses everything does not pass as
// markdown-free.
import { createRequire } from 'node:module'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { pathToFileURL } from 'node:url'

const out = path.resolve(process.argv[2] ?? '')
const markdown = JSON.stringify({ kind: 'renderMarkdown', text: '**hi**' })
const orchestration = JSON.stringify({ kind: 'orchestrationMay', agent: 'claude', action: 'merge' })
const ordinary = JSON.stringify({ kind: 'pluralCategory', locale: 'en', form: 'cardinal', count: 1 })

const nodePkg = createRequire(path.join(out, 'node', 'x.js'))('./astrid_rules_wasm.js')
const webPkg = await import(pathToFileURL(path.join(out, 'web', 'astrid_rules_wasm.js')).href)
webPkg.initSync({ module: readFileSync(path.join(out, 'web', 'astrid_rules_wasm_bg.wasm')) })

const failures = []
const ask = (name, pkg, request) => JSON.parse(pkg.runJson(request))
const expect = (cond, message) => { if (!cond) failures.push(message) }

const nodeMd = ask('node', nodePkg, markdown)
expect(nodeMd.ok === true, `node/ must answer renderMarkdown (astrid-web's server calls it): ${JSON.stringify(nodeMd)}`)
const webMd = ask('web', webPkg, markdown)
expect(webMd.ok === false && /unknown variant `renderMarkdown`/.test(webMd.error?.message ?? ''),
  `web/ must not carry the markdown renderer: ${JSON.stringify(webMd)}`)
const nodeOrch = ask('node', nodePkg, orchestration)
expect(nodeOrch.ok === true, `node/ must answer orchestrationMay (astrid-web's server asks it): ${JSON.stringify(nodeOrch)}`)
const webOrch = ask('web', webPkg, orchestration)
expect(webOrch.ok === false && /unknown variant `orchestrationMay`/.test(webOrch.error?.message ?? ''),
  `web/ must not carry the orchestration policy: ${JSON.stringify(webOrch)}`)
for (const [name, pkg] of [['node', nodePkg], ['web', webPkg]]) {
  const reply = ask(name, pkg, ordinary)
  expect(reply.ok === true, `${name}/ must answer an ordinary rule: ${JSON.stringify(reply)}`)
}

if (failures.length) {
  for (const f of failures) console.error(`check-wasm-packages: ${f}`)
  process.exit(1)
}
console.log('check-wasm-packages: node/ renders markdown and answers orchestration, web/ does neither; both answer the door')
