// Writes `strings/shared/` — the strings iOS and web both carry, hoisted into the core catalog
// (AITD-662, Jon's option 1).
//
// WHAT IT TAKES. Every astrid-ios key whose English is exactly a web key's English, once iOS's
// printf placeholders are put into web's named arguments by index. "Exactly" is deliberate: these
// are the strings the two clients already agree on word for word, so hoisting them changes no
// screen. A string the two say differently is a product decision about which one is right, and is
// not this script's to make.
//
// WHAT IT NAMES THEM. iOS's key in the core's dotted snake_case (`time.atDueTime` becomes
// `time.at_due_time`): divergences follow iOS, and the core's keys are snake_case. `sources.json`
// keeps the iOS key and the web keys each one came from, which is what a client's switch-over
// needs to re-point its lookups.
//
// WHERE THE WORDS COME FROM, per language: iOS's translation, then web's. A value iOS leaves in
// English never outranks a real translation from web, the rule `seed-strings.mjs` already has.
//
// WHAT IT LEAVES OUT, and lists: anything it cannot carry into all four formats without changing
// a word — ICU plurals and selects, a literal `%` (it means "format argument" to three of the four
// formats), a brace that is not an argument, an apostrophe before a brace (ICU reads it as
// quoting), a language with no usable value, and names that collide with the core's keys or with
// each other once the dots become underscores.
//
// It is a SEEDER, like `seed-strings.mjs`: run by hand, its output reviewed and committed, and
// `cargo xtask gen-strings` then generates the four formats from it.
//
//   node scripts/hoist-shared-strings.mjs [--check]
//
// `--check` writes nothing and prints the report. ASTRID_WEB_DIR / ASTRID_IOS_DIR point it at
// another checkout (see `catalogs.mjs`).

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

import { LOCALES, PRINTF, fromPrintf, iosCatalog, root, webCatalog } from './catalogs.mjs'

const check = process.argv.includes('--check')
const out = join(root, 'strings', 'shared')

// ── Names ───────────────────────────────────────────────────────────────────────────────────────

/** A dotted key every format can hold: segments of letters, digits and underscores. */
const KEY = /^[A-Za-z0-9_]+(\.[A-Za-z0-9_]+)*$/

/** iOS's key in the core's dotted snake_case. `tasks.taskId.copy` → `tasks.task_id.copy`. */
function snakeKey(key) {
  return key
    .split('.')
    .map((segment) =>
      segment
        .replace(/([A-Z]+)([A-Z][a-z])/g, '$1_$2')
        .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
        .toLowerCase(),
    )
    .join('.')
}

const flat = (key) => key.replace(/\./g, '_')

// ── Values ──────────────────────────────────────────────────────────────────────────────────────

const PLACEHOLDER = /\{([A-Za-z0-9_]+)\}/g

/** The `{name}`s of an ICU value, in order. */
const namesIn = (text) => [...text.matchAll(PLACEHOLDER)].map((match) => match[1])

/**
 * Why a value cannot be carried into all four formats unchanged, or `null` when it can.
 * `names` is the key's declared arguments; every one must appear and nothing else may.
 */
function unsafe(text, names) {
  if (/\{\s*[A-Za-z0-9_]+\s*,/.test(text)) return 'an ICU plural or select'
  if (text.includes('%')) return 'a literal %'
  if (/'[{}]/.test(text)) return 'an apostrophe before a brace'
  const rest = text.replace(PLACEHOLDER, '')
  if (/[{}]/.test(rest)) return 'a brace that is not an argument'
  const used = new Set(namesIn(text))
  if (used.size !== names.length || names.some((name) => !used.has(name))) {
    return 'arguments that differ from the English'
  }
  return null
}

/**
 * The iOS value in the key's named arguments, or `undefined`. Positions come from `names`, which
 * is indexed by printf position.
 */
function fromIos(value, names) {
  if (value === undefined) return undefined
  // `%%` is a literal percent, which `unsafe` refuses; catch it here, before `fromPrintf` turns it
  // into a bare `%` that a later step could mistake for something else.
  if (value.includes('%%')) return undefined
  const text = fromPrintf(value, names)
  if (text === undefined || unsafe(text, names.filter(Boolean))) return undefined
  return text
}

/**
 * The argument names an iOS English value takes from a web English value: the k-th placeholder
 * iOS writes gets the k-th name web writes, at iOS's printf position. `null` when they disagree in
 * number.
 */
function namesByPosition(iosEnglish, webEnglish) {
  const web = namesIn(webEnglish)
  const positions = []
  let next = 0
  for (const match of iosEnglish.matchAll(PRINTF)) {
    if (match[2] === '%') continue
    positions.push(match[1] === undefined ? next++ : Number(match[1]) - 1)
  }
  if (positions.length !== web.length) return null
  const names = []
  positions.forEach((position, k) => {
    names[position] = web[k]
  })
  if (names.length !== web.length || names.some((name) => name === undefined)) return null
  if (new Set(names).size !== names.length) return null
  return names
}

// ── The join ────────────────────────────────────────────────────────────────────────────────────

const ios = Object.fromEntries(LOCALES.map((locale) => [locale.tag, iosCatalog(locale)]))
const web = Object.fromEntries(LOCALES.map((locale) => [locale.tag, webCatalog(locale.web)]))
const translated = LOCALES.filter((locale) => locale.tag !== 'en').map((locale) => locale.tag)

if (Object.keys(ios.en).length === 0) {
  console.error('no astrid-ios Localizable.strings found; set ASTRID_IOS_DIR')
  process.exit(1)
}

const coreKeys = Object.keys(
  JSON.parse(readFileSync(join(root, 'strings', 'catalog.json'), 'utf8')).keys,
)
const coreFlat = new Map(coreKeys.map((key) => [flat(key), key]))

// Web keys by their English with the arguments blanked, so iOS's `%@` and web's `{list}` meet.
const shape = (text) => text.replace(PLACEHOLDER, '\u0000')
const webByShape = new Map()
for (const [key, text] of Object.entries(web.en)) {
  if (!text.trim()) continue
  const at = shape(text)
  if (!webByShape.has(at)) webByShape.set(at, [])
  webByShape.get(at).push(key)
}

const skipped = new Map() // reason → [iOS key]
const skip = (reason, key) => {
  if (!skipped.has(reason)) skipped.set(reason, [])
  skipped.get(reason).push(key)
}

const hoisted = new Map() // shared key → { ios, web, names, values, sources }
const flatTaken = new Map() // flat name → shared key

// Sorted, and keys already in snake_case first, so a collision keeps the name that needs no
// rename on iOS.
const iosKeys = Object.keys(ios.en).sort((a, b) => {
  const renamed = (key) => (snakeKey(key) === key ? 0 : 1)
  return renamed(a) - renamed(b) || a.localeCompare(b)
})

for (const iosKey of iosKeys) {
  const iosEnglish = ios.en[iosKey]
  if (!KEY.test(iosKey)) {
    skip('an iOS key that is not dotted words', iosKey)
    continue
  }
  if (iosEnglish.includes('%%')) {
    skip('a literal % in the English', iosKey)
    continue
  }
  const at = shape(iosEnglish.replace(PRINTF, (whole, _index, conversion) => (conversion === '%' ? '%' : '\u0000')))
  const candidates = (webByShape.get(at) ?? []).slice().sort()
  if (candidates.length === 0) continue // not shared: the ordinary case, not a skip

  // The first web key whose arguments line up with iOS's and whose English then matches exactly.
  let names = null
  let english = null
  for (const webKey of candidates) {
    const tried = namesByPosition(iosEnglish, web.en[webKey])
    if (!tried) continue
    const text = fromPrintf(iosEnglish, tried)
    if (text === web.en[webKey]) {
      names = tried
      english = text
      break
    }
  }
  if (names === null) {
    skip('arguments iOS and web cannot line up', iosKey)
    continue
  }
  const why = unsafe(english, names)
  if (why) {
    skip(`${why} in the English`, iosKey)
    continue
  }
  const webKeys = candidates.filter((webKey) => web.en[webKey] === english)

  const key = snakeKey(iosKey)
  if (coreKeys.includes(key)) {
    skip('already a core key', iosKey)
    continue
  }
  if (coreFlat.has(flat(key))) {
    skip(`flattens onto core key ${coreFlat.get(flat(key))}`, iosKey)
    continue
  }
  if (flatTaken.has(flat(key))) {
    skip(`flattens onto ${flatTaken.get(flat(key))}, already hoisted`, iosKey)
    continue
  }

  const values = { en: english }
  const sources = {}
  let missing = null
  for (const tag of translated) {
    const fromApp = fromIos(ios[tag][iosKey], names)
    const fromWeb = webKeys
      .map((webKey) => web[tag][webKey])
      .find((text) => text !== undefined && !unsafe(text, names))
    // iOS first; but an iOS value still in English loses to a real translation from web.
    if (fromApp !== undefined && (fromApp !== english || fromWeb === undefined || fromWeb === english)) {
      values[tag] = fromApp
      sources[tag] = 'ios'
    } else if (fromWeb !== undefined) {
      values[tag] = fromWeb
      sources[tag] = 'web'
    } else {
      missing = tag
      break
    }
  }
  if (missing) {
    skip(`no usable ${missing} value on either client`, iosKey)
    continue
  }

  hoisted.set(key, { ios: iosKey, web: webKeys, names, values, sources })
  flatTaken.set(flat(key), key)
}

// ── Report ──────────────────────────────────────────────────────────────────────────────────────

const keys = [...hoisted.keys()].sort()
const fromWebCount = keys.reduce(
  (sum, key) => sum + Object.values(hoisted.get(key).sources).filter((s) => s === 'web').length,
  0,
)
const englishOnly = keys.filter((key) => {
  const { values } = hoisted.get(key)
  return translated.every((tag) => values[tag] === values.en)
})
const renamed = keys.filter((key) => hoisted.get(key).ios !== key)

console.log(`iOS keys: ${Object.keys(ios.en).length}, web keys: ${Object.keys(web.en).length}`)
console.log(`hoisted: ${keys.length}`)
console.log(`  renamed to snake_case from iOS's spelling: ${renamed.length}`)
console.log(`  values from web where iOS had none or English: ${fromWebCount} of ${keys.length * translated.length}`)
console.log(`  the same in every language (deliberately English): ${englishOnly.length}`)
for (const key of englishOnly) console.log(`    ${key} = ${JSON.stringify(hoisted.get(key).values.en)}`)
console.log('\nleft out:')
for (const [reason, list] of [...skipped].sort((a, b) => b[1].length - a[1].length)) {
  console.log(`  ${reason}: ${list.length}`)
  for (const key of list.slice(0, 8)) console.log(`    ${key} = ${JSON.stringify(ios.en[key])}`)
  if (list.length > 8) console.log(`    … ${list.length - 8} more`)
}

if (check) process.exit(0)

// ── Writing it ──────────────────────────────────────────────────────────────────────────────────

mkdirSync(out, { recursive: true })
const json = (value) => `${JSON.stringify(value, null, 2)}\n`

writeFileSync(
  join(out, 'catalog.json'),
  json({
    note: 'GENERATED by scripts/hoist-shared-strings.mjs (AITD-662). The strings the clients share and the core does not emit. See strings/README.md.',
    keys: Object.fromEntries(
      keys.map((key) => {
        const { names } = hoisted.get(key)
        return [key, names.length ? { args: names } : {}]
      }),
    ),
  }),
)
writeFileSync(
  join(out, 'sources.json'),
  json({
    note: 'GENERATED by scripts/hoist-shared-strings.mjs. Each shared key\'s iOS key and the web keys with the same English: what a client re-points when it switches over.',
    keys: Object.fromEntries(
      keys.map((key) => [key, { ios: hoisted.get(key).ios, web: hoisted.get(key).web }]),
    ),
  }),
)
writeFileSync(
  join(out, 'provenance.json'),
  json({
    note: 'GENERATED by scripts/hoist-shared-strings.mjs. Every shared key rests on a shipped catalog (iOS or web). See strings/README.md.',
    harvested: keys.filter((key) => !englishOnly.includes(key)),
    authored: [],
    englishOnly,
  }),
)
for (const locale of LOCALES) {
  writeFileSync(
    join(out, `${locale.tag}.json`),
    json(Object.fromEntries(keys.map((key) => [key, hoisted.get(key).values[locale.tag]]))),
  )
}
console.log(`\nwrote ${keys.length} keys in ${LOCALES.length} locales to strings/shared/`)
