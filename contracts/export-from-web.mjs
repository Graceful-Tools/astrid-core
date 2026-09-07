#!/usr/bin/env node
// Derive cross-platform contract fixtures from the canonical astrid-web sources.
//
// The fixtures under contracts/fixtures/ are the ONLY thing the Rust tests compare against, and
// they are generated — never hand-edited. A rule that is retyped is a rule that drifts, which is
// the exact failure this whole mechanism exists to prevent.
//
// Usage:  node contracts/export-from-web.mjs [--web ../astrid-web] [--check]
//   --check exits non-zero if the generated output differs from what is on disk, which is what CI
//   runs; without it the files are written.
//
// This script is a stopgap in this repo. It moves to astrid-web as
// scripts/export-contract-fixtures.ts (plan §5.3) so the canonical repo owns the export and every
// client consumes the same artifacts; the output format will not change when it does.

import { readFileSync, writeFileSync, mkdirSync, existsSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { join, dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const args = process.argv.slice(2)
const check = args.includes('--check')
const webIndex = args.indexOf('--web')
const webRoot = resolve(here, '..', webIndex === -1 ? '../astrid-web' : args[webIndex + 1])

if (!existsSync(webRoot)) {
  console.error(`astrid-web checkout not found at ${webRoot} — pass --web <path>`)
  process.exit(2)
}

const read = (rel) => readFileSync(join(webRoot, rel), 'utf8')

// Display keys in the KEYBOARD_SHORTCUTS table vs the KeyboardEvent.key names the switch matches.
const EVENT_KEY = { '←': 'ArrowLeft', '→': 'ArrowRight', '↑': 'ArrowUp', '↓': 'ArrowDown' }

function exportShortcuts() {
  const src = read('hooks/useKeyboardShortcuts.ts')

  const tableMatch = src.match(/export const KEYBOARD_SHORTCUTS = \[([\s\S]*?)\n\] as const/)
  if (!tableMatch) throw new Error('KEYBOARD_SHORTCUTS table not found in useKeyboardShortcuts.ts')

  const rows = []
  const rowRe = /\{\s*key:\s*"([^"]+)",\s*description:\s*"([^"]*)",\s*action:\s*"([^"]+)"(?:,\s*param:\s*(\d+))?\s*\}/g
  for (const m of tableMatch[1].matchAll(rowRe)) {
    rows.push({ key: m[1], description: m[2], action: m[3], param: m[4] === undefined ? null : Number(m[4]) })
  }
  if (rows.length === 0) throw new Error('KEYBOARD_SHORTCUTS parsed to zero rows — the table shape changed')

  // The selection guard is not in the table; it is `if (selectedTask)` in the switch body. Walk the
  // switch so the guard is READ rather than assumed — it is half of the contract.
  // Bounded by markers rather than a shape-sensitive regex: indentation and line endings differ
  // between checkouts, and a brittle match here would fail as "contract missing" rather than
  // "parser stale", which is the worse of the two errors to be handed.
  const switchStart = src.indexOf('switch (key) {')
  const switchEnd = src.indexOf('}, [handlers', switchStart)
  if (switchStart === -1 || switchEnd === -1) {
    throw new Error('shortcut switch not found in useKeyboardShortcuts.ts - parser needs updating')
  }
  const switchBody = src.slice(switchStart, switchEnd)

  const guards = new Map()
  // Consecutive `case 'a':` labels share the body that follows, up to `break`.
  const caseRe = /((?:\s*case '[^']+':)+)([\s\S]*?)break/g
  for (const m of switchBody.matchAll(caseRe)) {
    const keys = [...m[1].matchAll(/case '([^']+)':/g)].map((c) => c[1])
    const requiresSelection = /if \(selectedTask\)/.test(m[2])
    for (const k of keys) guards.set(k, requiresSelection)
  }

  const shortcuts = rows.map((row) => {
    const eventKey = EVENT_KEY[row.key] ?? row.key
    if (!guards.has(eventKey)) {
      throw new Error(`key '${row.key}' (event '${eventKey}') is in the table but has no switch case`)
    }
    return {
      key: row.key,
      eventKey,
      description: row.description,
      // Matches the `onSetPriority(0)` convention the Mac table already uses for traceability.
      webAction: row.param === null ? row.action : `${row.action}(${row.param})`,
      requiresSelection: guards.get(eventKey),
    }
  })

  return { shortcuts }
}


// Repeating rollover is arithmetic, not a table, so the only honest way to lock it is to RUN the
// canonical implementation and record what it returns. A child process does it, with TZ pinned:
// web's custom-pattern path uses local date methods, so its results depend on the machine's
// timezone (docs/CONTRACTS.md D4) and an unpinned fixture would encode whoever generated it.
function exportRepeating() {
  const driver = join(here, 'drivers', 'repeating.mjs')
  const run = spawnSync(process.execPath, [driver, webRoot], {
    env: { ...process.env, TZ: 'UTC' },
    encoding: 'utf8',
    maxBuffer: 32 * 1024 * 1024,
  })
  if (run.status !== 0) {
    throw new Error(`repeating driver failed (exit ${run.status}):
${run.stderr}`)
  }
  return JSON.parse(run.stdout)
}

const EXPORTS = {
  'shortcuts.json': exportShortcuts,
  'repeating.json': exportRepeating,
}

let failed = false
mkdirSync(join(here, 'fixtures'), { recursive: true })
for (const [name, build] of Object.entries(EXPORTS)) {
  const payload = {
    $comment: 'GENERATED by contracts/export-from-web.mjs — do not edit. Source of truth: astrid-web.',
    ...build(),
  }
  const text = JSON.stringify(payload, null, 2) + '\n'
  const path = join(here, 'fixtures', name)
  if (check) {
    const current = existsSync(path) ? readFileSync(path, 'utf8') : ''
    if (current !== text) {
      console.error(`fixture out of date: contracts/fixtures/${name} (run node contracts/export-from-web.mjs)`)
      failed = true
    } else {
      console.log(`ok  contracts/fixtures/${name}`)
    }
  } else {
    writeFileSync(path, text)
    console.log(`wrote contracts/fixtures/${name}`)
  }
}
process.exit(failed ? 1 : 0)
