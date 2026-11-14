// One-off edit for task 3564db06: give the four `repeat.every_n_*` keys CLDR plural forms, and add
// `format.ordinal`, in all twelve languages. Kept beside `seed-strings.mjs` as the committed record
// of what was written and why, the same way `authored-translations.json` is.
//
// The singular arm may drop `{count}` where the language reads better without it — "Every day", not
// "Every 1 day". `crates/xtask/src/strings.rs` allows exactly that, and says why.
//
// Only categories a language actually spells differently are written. `plurals::category` can
// return `many` for the Romance millions rule; the text there is the same as `other`, and every
// renderer falls back to `other`, so a duplicate row would be noise.

import { readFileSync, writeFileSync } from 'node:fs'

const CADENCES = {
  en: {
    days: { one: 'Every day', other: 'Every {count} days' },
    weeks: { one: 'Every week', other: 'Every {count} weeks' },
    months: { one: 'Every month', other: 'Every {count} months' },
    years: { one: 'Every year', other: 'Every {count} years' },
  },
  de: {
    days: { one: 'Jeden Tag', other: 'Alle {count} Tage' },
    weeks: { one: 'Jede Woche', other: 'Alle {count} Wochen' },
    months: { one: 'Jeden Monat', other: 'Alle {count} Monate' },
    years: { one: 'Jedes Jahr', other: 'Alle {count} Jahre' },
  },
  es: {
    days: { one: 'Cada día', other: 'Cada {count} días' },
    weeks: { one: 'Cada semana', other: 'Cada {count} semanas' },
    months: { one: 'Cada mes', other: 'Cada {count} meses' },
    years: { one: 'Cada año', other: 'Cada {count} años' },
  },
  fr: {
    days: { one: 'Tous les jours', other: 'Tous les {count} jours' },
    weeks: { one: 'Toutes les semaines', other: 'Toutes les {count} semaines' },
    months: { one: 'Tous les mois', other: 'Tous les {count} mois' },
    years: { one: 'Tous les ans', other: 'Tous les {count} ans' },
  },
  it: {
    days: { one: 'Ogni giorno', other: 'Ogni {count} giorni' },
    weeks: { one: 'Ogni settimana', other: 'Ogni {count} settimane' },
    months: { one: 'Ogni mese', other: 'Ogni {count} mesi' },
    years: { one: 'Ogni anno', other: 'Ogni {count} anni' },
  },
  pt: {
    days: { one: 'Todos os dias', other: 'A cada {count} dias' },
    weeks: { one: 'Todas as semanas', other: 'A cada {count} semanas' },
    months: { one: 'Todos os meses', other: 'A cada {count} meses' },
    years: { one: 'Todos os anos', other: 'A cada {count} anos' },
  },
  nl: {
    days: { one: 'Elke dag', other: 'Elke {count} dagen' },
    weeks: { one: 'Elke week', other: 'Elke {count} weken' },
    months: { one: 'Elke maand', other: 'Elke {count} maanden' },
    years: { one: 'Elk jaar', other: 'Elke {count} jaar' },
  },
  // Russian has three integer categories, and this is the one language of the twelve that does
  // not need them: the wording landed on main while this task was in flight — "Каждый {count}-й
  // день", an ordinal adjective — is invariant. "Каждый 21-й день" is as correct as "каждый 3-й".
  //
  // So it gets one form. A plural key still has to BE plural in every language (the catalog's own
  // rule: whether a key has categories is the key's business, not a language's), and `other` is the
  // category every renderer falls back to, so one `other` row is the honest shape rather than three
  // rows spelling the same words.
  ru: {
    days: { other: 'Каждый {count}-й день' },
    weeks: { other: 'Каждую {count}-ю неделю' },
    months: { other: 'Каждый {count}-й месяц' },
    years: { other: 'Каждый {count}-й год' },
  },
  // The four with no plural categories at all. One form, carrying the number — which is correct,
  // and is what a `count == 1` shell suppresses.
  ja: {
    days: { other: '{count}日ごと' },
    weeks: { other: '{count}週間ごと' },
    months: { other: '{count}か月ごと' },
    years: { other: '{count}年ごと' },
  },
  ko: {
    days: { other: '{count}일마다' },
    weeks: { other: '{count}주마다' },
    months: { other: '{count}개월마다' },
    years: { other: '{count}년마다' },
  },
  'zh-CN': {
    days: { other: '每 {count} 天' },
    weeks: { other: '每 {count} 周' },
    months: { other: '每 {count} 个月' },
    years: { other: '每 {count} 年' },
  },
  'zh-TW': {
    days: { other: '每 {count} 天' },
    weeks: { other: '每 {count} 週' },
    months: { other: '每 {count} 個月' },
    years: { other: '每 {count} 年' },
  },
}

// How each language spells a number as a position. English is the only one of the twelve with more
// than two forms, and it is the one every client has hand-rolled.
const ORDINALS = {
  en: { one: '{count}st', two: '{count}nd', few: '{count}rd', other: '{count}th' },
  de: { other: '{count}.' },
  es: { other: '{count}' },
  fr: { one: '{count}er', other: '{count}e' },
  it: { other: '{count}º' },
  pt: { other: '{count}º' },
  nl: { other: '{count}e' },
  ru: { other: '{count}' },
  ja: { other: '{count}' },
  ko: { other: '{count}' },
  'zh-CN': { other: '{count}' },
  'zh-TW': { other: '{count}' },
}

// Japanese numbers a weekday within a month with 第, which the template was missing: `{ordinal}`
// resolving to a bare "3" left "3火曜日に" where "第3火曜日に" is the Japanese for it. Part of the
// acceptance — Japanese repeat summaries are grammatical — rather than a drive-by.
const CORRECTIONS = {
  ja: { 'repeat.on_nth_weekday': '第{ordinal}{weekday}に' },
}

for (const [locale, cadences] of Object.entries(CADENCES)) {
  const path = `strings/${locale}.json`
  const words = JSON.parse(readFileSync(path, 'utf8'))
  for (const [unit, forms] of Object.entries(cadences)) {
    words[`repeat.every_n_${unit}`] = forms
  }
  words['format.ordinal'] = ORDINALS[locale]
  for (const [key, text] of Object.entries(CORRECTIONS[locale] ?? {})) {
    words[key] = text
  }
  const sorted = {}
  for (const key of Object.keys(words).sort()) sorted[key] = words[key]
  writeFileSync(path, JSON.stringify(sorted, null, 2) + '\n')
  console.log(`${path}: ${Object.keys(sorted).length} keys`)
}

// `format.ordinal` is written for this catalog, so provenance records it as authored — it wants a
// native speaker's pass, like every other key in that list. The four cadence keys are already
// there, and giving them plural forms does not change where their translations came from.
const provenancePath = 'strings/provenance.json'
const provenance = JSON.parse(readFileSync(provenancePath, 'utf8'))
if (!provenance.authored.includes('format.ordinal')) {
  provenance.authored.push('format.ordinal')
  provenance.authored.sort()
}
writeFileSync(provenancePath, JSON.stringify(provenance, null, 2) + '\n')
console.log(`${provenancePath}: ${provenance.authored.length} authored`)
