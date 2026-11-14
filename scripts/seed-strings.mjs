// Seeds `strings/` — the core's own catalog — from the catalogs that already exist (task 1bdf84d7).
//
// WHY THIS IS A SCRIPT AND NOT A PASTE. The twelve languages in `strings/<locale>.json` are not
// new translations: astrid-web already carries 12 locales over ~970 keys, and astrid-windows'
// `Strings.cs` fallback table is the only place the ENGLISH of the core's own keys is written
// down. So the catalog is seeded by joining the two — core key → its English → the web key with
// the same English → that key's twelve translations. Doing that by hand once would leave nobody
// able to check it or to run it again when the next tranche arrives, so it is here, it is
// committed, and it prints what it could not find rather than quietly writing English.
//
// It is a SEEDER, not a generator. `cargo xtask gen-strings` generates the four platforms' output
// FROM `strings/`; this writes `strings/` itself, is run by hand, and its output is then reviewed
// and committed like any other data. Re-running it overwrites translations, so once a human or a
// translator has corrected one, this stops being the right tool for that key — which is why it
// reports every key it filled and from where.
//
//   node scripts/seed-strings.mjs [--check]
//
// `--check` writes nothing and only prints the coverage report.

import { execFileSync } from 'node:child_process'
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const web = join(root, '..', 'astrid-web')
const windows = join(root, '..', 'astrid-windows')
const out = join(root, 'strings')
const check = process.argv.includes('--check')

// ── The twelve languages, and what each platform calls them ─────────────────────────────────────
//
// One table, because the four platforms disagree about every one of the interesting ones and a
// mapping written twice is a mapping that disagrees with itself. `zh-CN` is `zh-Hans` to Apple,
// `values-zh-rCN` to Android and `zh-Hans-CN` to Windows; Android's English is the unqualified
// `values` directory, not `values-en`.
const LOCALES = [
  { tag: 'en', name: 'English', web: 'en', apple: 'en', android: 'values', windows: 'en-US' },
  { tag: 'es', name: 'Español', web: 'es', apple: 'es', android: 'values-es', windows: 'es-ES' },
  { tag: 'fr', name: 'Français', web: 'fr', apple: 'fr', android: 'values-fr', windows: 'fr-FR' },
  { tag: 'de', name: 'Deutsch', web: 'de', apple: 'de', android: 'values-de', windows: 'de-DE' },
  { tag: 'it', name: 'Italiano', web: 'it', apple: 'it', android: 'values-it', windows: 'it-IT' },
  { tag: 'pt', name: 'Português', web: 'pt', apple: 'pt-PT', android: 'values-pt', windows: 'pt-PT' },
  { tag: 'nl', name: 'Nederlands', web: 'nl', apple: 'nl', android: 'values-nl', windows: 'nl-NL' },
  { tag: 'ru', name: 'Русский', web: 'ru', apple: 'ru', android: 'values-ru', windows: 'ru-RU' },
  { tag: 'ja', name: '日本語', web: 'ja', apple: 'ja', android: 'values-ja', windows: 'ja-JP' },
  { tag: 'ko', name: '한국어', web: 'ko', apple: 'ko', android: 'values-ko', windows: 'ko-KR' },
  { tag: 'zh-CN', name: '简体中文', web: 'zh-CN', apple: 'zh-Hans', android: 'values-zh-rCN', windows: 'zh-Hans-CN' },
  { tag: 'zh-TW', name: '繁體中文', web: 'zh-TW', apple: 'zh-Hant', android: 'values-zh-rTW', windows: 'zh-Hant-TW' },
]

// ── What each key interpolates, IN ORDER ────────────────────────────────────────────────────────
//
// The order is a contract, not something a generator can infer. `.resw` writes `{0} {1}`, Android
// `%1$s %2$s` and Apple `%1$@ %2$@`, all of which are positional — so a language that reorders the
// words still has to mark which argument is which, and the only way that works is if every format
// agrees on what argument 1 is. These orders are astrid-windows' `Converters.cs` `Say()`, which is
// the only shell that assembles the repeat summary today; changing one would silently reorder a
// sentence in a shipped build.
//
// The names are the catalog's own. The core sends a `SummaryPart` carrying `count`, `values` and
// `date`, and the shell turns a count into an ordinal and a wire weekday into a word before it
// formats — so `ordinal` and `weekday` say what the argument IS rather than which field carried it.
const ARGS = {
  'repeat.every_n_days': ['count'],
  'repeat.every_n_weeks': ['count'],
  'repeat.every_n_months': ['count'],
  'repeat.every_n_years': ['count'],
  'repeat.on_weekdays': ['weekdays'],
  'repeat.on_day_of_month': ['ordinal'],
  'repeat.on_nth_weekday': ['ordinal', 'weekday'],
  'repeat.on_last_weekday': ['weekday'],
  'repeat.on_month_and_day': ['month', 'ordinal'],
  'repeat.ends_after': ['count'],
  'repeat.ends_on': ['date'],
  'tasks.addTaskToList': ['list'],
}

// ── English the core needs and no catalog anywhere carries ──────────────────────────────────────
//
// Four of the quick-add prompts are exactly the failure this task exists to end: `placeholder()`
// returns `tasks.addTaskToCurrentList`, and that key is in no catalog on any platform, so every
// client that asked for it would draw the key. Their English is authored HERE, which is now the
// right place for it — and they are the four keys a translator has to be given, since there is
// nothing to harvest their twelve languages from.
// The repeat FILTER's five rows are the other half of it, and they are a sharper find: the filter
// sheet asks for `repeat.daily` while the repeat PICKER asks for `repeating.daily`, two keys for
// one word. `Strings.cs` carries the second and not the first, so astrid-windows' filter sheet
// draws the literal text `repeat.daily` on screen today. Catalogued under both spellings here,
// because the catalog's job is to make every key the core emits sayable; making the core emit one
// of them is a change to a pinned contract and is filed separately.
const AUTHORED = {
  'tasks.addNewTask': 'Add a new task',
  'tasks.addTaskShort': 'Add task',
  'tasks.addTaskToCurrentList': 'Add a task to this list',
  'tasks.addTaskToList': 'Add a task to {list}',
  'repeat.daily': 'Daily',
  'repeat.weekly': 'Weekly',
  'repeat.monthly': 'Monthly',
  'repeat.yearly': 'Yearly',
  'repeat.custom': 'Custom',
}

// Translations written for this catalog rather than harvested, because no catalog on any reachable
// platform carries the key. Kept as data in a file of its own so that a native speaker correcting
// one is editing a reviewable input rather than a generated artefact — re-running this seeder
// keeps their correction.
const authoredPath = join(root, 'scripts', 'authored-translations.json')

// ── Reading the sources ─────────────────────────────────────────────────────────────────────────

/** Every key the core emits, asked of the core rather than listed here. */
function coreKeys() {
  return execFileSync('cargo', ['run', '-q', '-p', 'xtask', '--', 'strings-keys'], {
    cwd: root,
    encoding: 'utf8',
  })
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean)
}

/**
 * astrid-windows' `Strings.cs` fallback table, which is where the core keys' English lives.
 *
 * Parsed rather than copied, so re-running this picks up a correction made in the shell. The
 * entries are `["key"] = "text",` with C# escapes; only `\"` and `\\` occur.
 */
function windowsEnglish() {
  const source = readFileSync(join(windows, 'app/Astrid.App/Strings.cs'), 'utf8')
  const table = {}
  for (const match of source.matchAll(/\["([^"]+)"\]\s*=\s*"((?:[^"\\]|\\.)*)"/g)) {
    table[match[1]] = match[2].replace(/\\"/g, '"').replace(/\\\\/g, '\\')
  }
  return table
}

/**
 * One of astrid-windows' `.resw` files, keyed back by the core's dotted keys.
 *
 * The shell ships two languages, and its German is a real translation of exactly the keys this
 * catalog is about — the only translated copy of them that is reachable from here at all. The
 * resource names are the dotted keys with underscores (`Strings.Get` makes that swap), so the only
 * way back is to ask which core key underscores to this name.
 */
function windowsResw(language, keys) {
  const path = join(windows, 'app/Astrid.App/Strings', language, 'Resources.resw')
  let source
  try {
    source = readFileSync(path, 'utf8')
  } catch {
    return {}
  }
  const byName = {}
  for (const match of source.matchAll(
    /<data name="([^"]+)"[^>]*>\s*<value>([\s\S]*?)<\/value>/g,
  )) {
    byName[match[1]] = match[2]
      .replace(/&lt;/g, '<')
      .replace(/&gt;/g, '>')
      .replace(/&quot;/g, '"')
      .replace(/&apos;/g, "'")
      .replace(/&amp;/g, '&')
  }
  const table = {}
  for (const key of keys) {
    const name = key.replace(/\./g, '_')
    if (byName[name] !== undefined) table[key] = byName[name]
  }
  return table
}

/** One of web's next-intl catalogs, flattened to the dotted keys next-intl looks them up by. */
function webCatalog(locale) {
  const nested = JSON.parse(readFileSync(join(web, 'lib/i18n/locales', `${locale}.json`), 'utf8'))
  const flat = {}
  const walk = (node, prefix) => {
    for (const [key, value] of Object.entries(node)) {
      const path = prefix ? `${prefix}.${key}` : key
      if (value && typeof value === 'object') walk(value, path)
      else if (typeof value === 'string') flat[path] = value
    }
  }
  walk(nested, '')
  return flat
}

/**
 * The form two strings are compared in when asking "is this the same words".
 *
 * Case, surrounding space, a trailing ellipsis and the placeholder syntax all differ between the
 * catalogs for reasons that are not about meaning: Windows writes `Custom…` and `{0}`, web writes
 * `Custom...` and `{count}`. Comparing those raw would match almost nothing.
 */
function sameWords(text) {
  return text
    .replace(/\{[^}]*\}/g, ' ')
    .replace(/[.…]+$/u, '')
    .replace(/\s+/g, ' ')
    .trim()
    .toLowerCase()
}

// ── The join ────────────────────────────────────────────────────────────────────────────────────

const keys = coreKeys()
const fromWindows = windowsEnglish()
const catalogs = Object.fromEntries(LOCALES.map((locale) => [locale.tag, webCatalog(locale.web)]))
// The shell's own German, which is a translation of these exact keys. It wins over a words-match
// against web, and loses to nothing.
const fromWindowsGerman = windowsResw('de-DE', keys)
let authoredTranslations = {}
try {
  authoredTranslations = JSON.parse(readFileSync(authoredPath, 'utf8'))
} catch {
  // Not there yet on a first run; the report below says how many keys want one.
}

// English text → every web key that says it. A list, because web says "Priority" under four keys
// and they need not be translated identically.
const byEnglish = new Map()
for (const [key, text] of Object.entries(catalogs.en)) {
  const words = sameWords(text)
  if (!words) continue
  if (!byEnglish.has(words)) byEnglish.set(words, [])
  byEnglish.get(words).push(key)
}

/** How close a web key is to a core key, for choosing between several that say the same words. */
function affinity(coreKey, webKey) {
  const left = coreKey.split('.')
  const right = webKey.split('.')
  let shared = 0
  for (const segment of left) if (right.includes(segment)) shared += 1
  // A leaf that matches is worth more than a namespace that does: `filter.due.today` should prefer
  // web's `...dueToday` over anything else merely sitting under `filter`.
  if (right[right.length - 1].toLowerCase() === left[left.length - 1].replace(/_/g, '')) shared += 2
  return shared
}

const english = {}
const translations = Object.fromEntries(LOCALES.map((locale) => [locale.tag, {}]))
const report = { authored: [], exact: [], matched: [], englishOnly: [], noEnglish: [] }

for (const key of keys) {
  // 1. The English. Authored here where nothing carries it, else the shell's own table, else web's
  //    key of the same name (the `tasks.*` keys are web's spelling already).
  const text = AUTHORED[key] ?? fromWindows[key] ?? catalogs.en[key]
  if (text === undefined) {
    report.noEnglish.push(key)
    continue
  }
  english[key] = text

  // 2. The other eleven. A key web carries under the same NAME is the strongest evidence there is;
  //    otherwise the web key that says the same words in English carries the same words in Russian.
  let source = null
  if (catalogs.en[key] !== undefined) {
    source = key
    report.exact.push(key)
  } else {
    const candidates = byEnglish.get(sameWords(text)) ?? []
    if (candidates.length > 0) {
      source = candidates.slice().sort((a, b) => affinity(key, b) - affinity(key, a))[0]
      report.matched.push(`${key} ← ${source}`)
    }
  }

  // 3. Fill each language from the best evidence there is for it, in this order:
  //      the shell's own translation of THIS key > web's, by key or by words > one written for
  //      this catalog > English.
  //    The shell's German leads because it is a translation of the very key being asked about; a
  //    words-match against web is a translation of the same English under a different key, which
  //    is nearly as good and is still somebody's work rather than a guess.
  let harvested = 0
  for (const locale of LOCALES) {
    if (locale.tag === 'en') {
      translations.en[key] = text
      continue
    }
    const fromShell = locale.tag === 'de' ? fromWindowsGerman[key] : undefined
    const fromWeb = source === null ? undefined : catalogs[locale.tag][source]
    const written = authoredTranslations[key]?.[locale.tag]
    const value = fromShell ?? fromWeb ?? written ?? text
    if (value !== text) harvested += 1
    // English is the fallback, and a visible one: the report below counts every value that is
    // still English, so "untranslated" is a number somebody can act on rather than a silence.
    translations[locale.tag][key] = value
  }

  if (source !== null) {
    // already reported as exact or matched
  } else if (harvested > 0) {
    report.matched.push(`${key} ← astrid-windows de-DE / authored`)
  } else if (AUTHORED[key]) {
    report.authored.push(key)
  } else {
    report.englishOnly.push(key)
  }
}

// Placeholders: the core's catalog is ICU with NAMED arguments, so the `{0}` the Windows table
// carries, and web's own argument names, are rewritten into this key's names — BY POSITION, since
// that is the only thing a `{0}` carries.
//
// A message that ALREADY uses this key's names is left exactly alone, and that exception is the
// whole correctness of this step rather than a nicety. A translation is free to reorder the
// arguments — Spanish writes `el {ordinal} de {month}` where English writes `on {month}
// {ordinal}` — so rewriting by position would rename `ordinal` to `month` and produce "el 3 de
// marzo" as "el marzo de 3". It did exactly that before this check existed.
const renameByPosition = (value, names) => {
  const used = [...value.matchAll(/\{([^}]*)\}/g)].map((match) => match[1])
  if (used.length === 0) return value
  if (used.every((name) => names.includes(name))) return value
  let index = 0
  return value.replace(/\{[^}]*\}/g, () => `{${names[index++] ?? 'arg'}}`)
}
for (const [key, names] of Object.entries(ARGS)) {
  for (const locale of LOCALES) {
    const value = translations[locale.tag][key]
    if (value === undefined) continue
    translations[locale.tag][key] = renameByPosition(value, names)
  }
  if (english[key] !== undefined) english[key] = renameByPosition(english[key], names)
}

// ── What it found ───────────────────────────────────────────────────────────────────────────────

// A value identical to English is untranslated — unless the authored file says that IS the
// translation. `({count}x)` is written the same way in every one of the twelve, and counting it as
// a gap would leave a number nobody could ever drive to zero.
const deliberate = (key, tag) => authoredTranslations[key]?.[tag] !== undefined
const stillEnglish = {}
for (const locale of LOCALES) {
  if (locale.tag === 'en') continue
  stillEnglish[locale.tag] = keys.filter(
    (key) =>
      english[key] !== undefined &&
      translations[locale.tag][key] === english[key] &&
      !deliberate(key, locale.tag),
  )
}
const untranslated = Object.fromEntries(
  Object.entries(stillEnglish).map(([tag, list]) => [tag, list.length]),
)

console.log(`keys the core emits: ${keys.length}`)
console.log(`  English from the shell table or web, by name: ${report.exact.length}`)
console.log(`  English matched to a web key by its words:    ${report.matched.length}`)
console.log(`  English authored here (no catalog has it):    ${report.authored.length}`)
console.log(`  English found but no translations:            ${report.englishOnly.length}`)
if (report.noEnglish.length > 0) {
  console.log(`\nNO ENGLISH AT ALL (${report.noEnglish.length}):`)
  for (const key of report.noEnglish) console.log(`  ${key}`)
}
if (report.englishOnly.length > 0) {
  console.log(`\nEnglish only, needs a translator (${report.englishOnly.length}):`)
  for (const key of report.englishOnly) console.log(`  ${key}`)
}
console.log('\nstill English per locale:')
for (const [tag, count] of Object.entries(untranslated)) console.log(`  ${tag}: ${count}`)

if (check) process.exit(report.noEnglish.length > 0 ? 1 : 0)

// ── Writing it ──────────────────────────────────────────────────────────────────────────────────

const stable = (table) =>
  Object.fromEntries(
    Object.keys(table)
      .sort()
      .map((key) => [key, table[key]]),
  )

mkdirSync(out, { recursive: true })

const catalog = {
  note: 'GENERATED first by scripts/seed-strings.mjs, then maintained by hand. See strings/README.md.',
  keys: stable(
    Object.fromEntries(
      Object.keys(english).map((key) => [key, ARGS[key] ? { args: ARGS[key] } : {}]),
    ),
  ),
}
writeFileSync(join(out, 'catalog.json'), `${JSON.stringify(catalog, null, 2)}\n`)

// Where each key's twelve languages CAME from, so the next person can tell a shipped translation
// from one written for this catalog without reading the seeder. `check-strings` asserts the three
// lists partition the catalog, which is what stops a key quietly losing its provenance when it
// gains a translation.
const provenance = {
  note: 'GENERATED by scripts/seed-strings.mjs. Which evidence each key\'s translations rest on. See strings/README.md.',
  harvested: Object.keys(english)
    .filter((key) => !authoredTranslations[key] && untranslated[LOCALES[1].tag] !== undefined)
    .filter((key) => !stillEnglish[LOCALES[1].tag].includes(key))
    .sort(),
  authored: Object.keys(english)
    .filter((key) => authoredTranslations[key] !== undefined)
    .sort(),
  englishOnly: Object.keys(english)
    .filter(
      (key) =>
        authoredTranslations[key] === undefined && stillEnglish[LOCALES[1].tag].includes(key),
    )
    .sort(),
}
writeFileSync(join(out, 'provenance.json'), `${JSON.stringify(provenance, null, 2)}\n`)
writeFileSync(join(out, 'locales.json'), `${JSON.stringify({ locales: LOCALES }, null, 2)}\n`)
for (const locale of LOCALES) {
  writeFileSync(
    join(out, `${locale.tag}.json`),
    `${JSON.stringify(stable(translations[locale.tag]), null, 2)}\n`,
  )
}
console.log(`\nwrote ${LOCALES.length} locales, catalog.json and locales.json to strings/`)
