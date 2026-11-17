# `strings/` — one catalog, four platforms

**The words for every key this core asks a client to say**, in the twelve languages the product
ships, generated into web's, Apple's, Android's and Windows' own formats. Beside them, in
`shared/`, are **the strings the clients already carry in common** and no core function returns
(AITD-662), so that a word shared by every client is written and translated once.

Task `1bdf84d7`, from the cross-client review of 2026-11-11 (`docs/CROSS_CLIENT_REVIEW_2026-11.md`).

## Why

Rule 10 is that nothing in the core returns English. A due pick comes back as `picker.tomorrow`
and the shell says the word. That rule had a hole in it: **nothing said what the whole set of keys
was.** There were four hand-maintained catalogs in four naming conventions —

| Catalog | Keys | Languages | Key style |
|---|---|---|---|
| web | 937 | 12 | camelCase |
| iOS | 1,421 | 12 | snake_case |
| Android | 848 | ported from iOS by a script | `group_leaf` |
| Windows | 736 | 2 | `x:Uid` |

— and the only shared record of the core's *own* keys was a 13-row table in astrid-web's
`docs/PRODUCT_CONTRACT.md` §2, a hand-copied array in astrid-windows'
`CorePinFollowsIosTests.cs`, and an Android file generated from astrid-windows' `.resw`: a copy of
a copy.

So a key added here reached a client as a key drawn on screen. That compiles, passes every test,
and looks like a rendering bug. **Four of the keys in this catalog were in exactly that state when
it was written** — `placeholder()` has returned `tasks.addTaskToCurrentList` for months and no
catalog on any platform has ever carried it.

## The files

| File | What it is |
|---|---|
| `catalog.json` | the schema: every key, the arguments it interpolates **in order**, and whether it pluralises |
| `<locale>.json` | the words, one file per language, keyed by the core's own dotted keys |
| `locales.json` | the twelve languages and what each platform calls each of them |
| `provenance.json` | which evidence each key's translations rest on — see *Translations*, below |
| `shared/` | the second section, the same files over again (`catalog.json`, `<locale>.json`, `provenance.json`) plus `sources.json` — see *The shared section*, below |
| `generated/` | `cargo xtask gen-strings` output, both sections in one file per platform and language. **Do not edit**; `check-strings` regenerates and compares |

`cargo xtask check-strings` is the gate, and it is in CI. It fails when a key this core emits is
not in the catalog, when a key in the catalog is no longer emitted, when a language is missing
words, when a translation interpolates an argument the schema does not declare — and when
`generated/` is not what the source says. Since AITD-662 it also fails when a key is in both
sections, when a key the core emits sits in the shared section, and when two keys flatten to the
same resource name (`a.b_c` and `a_b.c` are one name on web, Android and Windows).

`crates/astrid-rules/tests/strings_catalog.rs` asks the same question inside `cargo test`, plus one
more that only a source scan can: that no key-shaped literal anywhere in `astrid-rules` has escaped
`copy::keys()` — the backstop for a new module nobody remembered to register.

## The keys

`astrid_rules::copy::keys()`, re-exported as `astrid_core::copy_keys()` beside `command_kinds()`.
The two together are the whole vocabulary a client has to implement.

It is **computed** from the same const tables and functions the pickers themselves call, never a
second hand-written list — a second list is a second place to add a key, which is the failure this
closes.

### One key is not dotted

`sort` is the sort group's own title, while its choices are `sort.auto`, `sort.priority` and the
rest. Every other group's title shares its choices' prefix. It is pinned as the single exception in
`copy.rs` rather than tidied, because astrid-windows' shipped `Strings.cs` looks it up under that
name and a pinned client cannot be fixed by editing this repository.

### Two keys for one word, until they were one

The repeat **filter** asked for `repeat.daily` while the repeat **picker** asked for
`repeating.daily` — five words under two spellings, which this catalog found by being the first
thing to look at every key the core emits at once. Both were carried here, because a catalog's job
is to make every key the core emits sayable whatever is decided about it.

Resolved in task `b0b5f3cc`: the filter took the picker's keys (`docs/CONTRACTS.md` D60) and the
five `repeat.*` spellings left this catalog. The check that made the pruning compulsory is the one
below — a key in the catalog the core no longer emits is a failure, not dead weight.

## The shared section

`shared/` holds the strings iOS and web both say, word for word, that no core function returns:
"Close", "Copy task id", "Comments ({count})". Jon chose this shape on AITD-662: one catalog, so
nothing is translated twice, while "every key here is one the core emits" stays true of the core
section.

The two sections are held to the same checks (twelve languages, declared arguments, plural shape,
provenance) with **one exemption**: a shared key need not be emitted by this core, because it is a
client's word. That exemption is also why a key the core *does* emit is refused in `shared/`: left
there, it would escape the check that the core's own keys are catalogued.

They are in files of their own, not inside `catalog.json`, because `scripts/seed-strings.mjs`
rewrites the core section's files from scratch.

`scripts/hoist-shared-strings.mjs` writes it:

- **What it takes.** Every astrid-ios key whose English is exactly a web key's English, with iOS's
  printf placeholders put by index into web's named arguments. Only exact matches, so hoisting
  changes no screen. Strings the clients word differently need a product decision first.
- **How it names them.** iOS's key in dotted snake_case (`time.atDueTime` → `time.at_due_time`),
  since divergences follow iOS. `sources.json` records each key's iOS key and the web keys with the
  same English: the list a client works through when it switches over.
- **Where the words come from.** iOS's translation, then web's; an iOS value still in English loses
  to a real translation from web. So `provenance.json` here is all **harvested** (one key, an
  example name, is the same in every language on purpose).
- **What it leaves out, and says so.** ICU plurals and selects, a literal `%`, a brace that is not
  an argument, an apostrophe before a brace, a language neither client can supply, and names that
  collide with a core key or with each other once flattened.

First run (2026-11-14, astrid-ios `main` and astrid-web `main`): **564 keys**, 39 of them with
arguments. Left out: 18 already core keys, 6 iOS keys that are English sentences rather than
dotted names ("New Task"), and 2 that flatten onto a key already hoisted. Run it again as the
clients converge; it rewrites `shared/` whole.

A shared argument reaches Apple as `%n$@` like every other, so an iOS lookup that passed an `Int`
for a `%d` passes a string once it switches over.

## Key spelling per platform

The core's keys cannot be nested: `filter.due` is a filter's own title **and** the parent of
`filter.due.today`. A JSON tree cannot hold a string and an object at one path, next-intl resolves
`t('a.b')` by walking the tree, and the Windows PRI compiler refuses a flat resource whose name is
also a scope.

| Platform | Key | Why |
|---|---|---|
| web | `filter_due_today` | next-intl walks `.` as a path |
| Android | `filter_due_today` | a resource id is `[A-Za-z0-9_]` |
| Windows | `filter_due_today` | the PRI compiler refuses a name that is also a scope — the swap `Strings.Get` already makes |
| Apple | `filter.due.today` | `.strings` keys are opaque, so the core's own spelling survives |

## Arguments

The catalog is ICU with **named** arguments — `Every {count} days`, `on the {ordinal} {weekday}`.
Three of the four formats have no names (`.resw` is `{0}`, Android `%1$s`, Apple `%1$@`), so
`catalog.json` declares each key's arguments **in order** and that order is the contract. It is
astrid-windows' `Converters.cs`, the only shell assembling a repeat summary today.

A translation is free to **reorder** them — Spanish writes `el {ordinal} de {month}` where English
writes `on {month} {ordinal}` — and the positional forms are numbered for exactly that reason:
that key renders as `el %2$s de %1$s` on Android. Reordering is why the order is declared rather
than inferred from the English, and the seeder has a test's worth of comment about getting this
wrong once.

## Plurals

A locale's value may be an object of CLDR categories instead of a string, with the key marked
`"plural": true`. Every format renders it natively: ICU inline for web, `<plurals>` for Android, a
`.stringsdict` for Apple, and one flat resource per category for Windows, which has no plural
resource at all.

**Five keys declare one** (task `3564db06`): the four `repeat.every_n_*` cadences and
`format.ordinal`. They were the candidates this support was built for — both iOS and Windows
special-cased a count of one in the shell, with a separate `repeat.every_day` string, which is
English's and German's rule applied to Russian's three forms and to Japanese's none.

Which category a number selects is **not** in this directory. It is a rule, so it is
`astrid_rules::plurals::category(locale, form, count)`, transcribed from CLDR for these twelve
languages and asserted in `crates/astrid-rules/src/plurals.rs`. The catalog holds the words; the
rule picks the row.

### Ordinals are a second rule set

`"ordinal": true` beside `"plural": true` says a key's categories are CLDR's **ordinal** ones.
English is the only one of the twelve with more than two (`1st`, `2nd`, `3rd`, `4th` — and `11th`,
which is why this is a rule and not a lookup of the last digit).

Only web can select one natively, with ICU's `selectordinal`; `plural` there would answer `other`
for English's 3 and write "3th". The other three formats have no ordinal resource at all —
`<plurals>`' `quantity` is cardinal, `NSStringPluralRuleType` is Apple's only rule type, and a
`.resw` has nothing — so an ordinal key reaches them as **one flat resource per category**
(`format_ordinal_few`, `"format.ordinal.few"`) and the client picks with `plurals::category`.

`repeat.on_day_of_month`, `repeat.on_nth_weekday` and `repeat.on_month_and_day` interpolate the
result as their `{ordinal}` argument, which is why those three keys did not change: what moved is
where "3rd" comes from. The summary part itself now says which kind of number it carries — see
`SummaryPart::form`.

## Translations

`scripts/seed-strings.mjs` wrote the first version of this catalog, and can be re-run. It joins
what already exists rather than inventing anything: the core key's English (astrid-windows'
`Strings.cs` fallback table, the only place it was written down), then whatever the other catalogs
say for that English in the other eleven languages.

Each language is filled from the best evidence there is for it, in this order:

1. **astrid-windows' German `.resw`**, a translation of these very keys;
2. **astrid-ios's `Localizable.strings` under the same key**, but only while iOS's English says the
   same words: iOS reuses some of these names for other text (its `picker.morning` is
   "Morning (9 AM)");
3. **astrid-web's key of the same name, or the web key whose English says the same words**;
4. **the iOS key whose English says the same words**, or one of the few listed in the seeder's
   `IOS_ALIASES`, which say the same thing in other English ("1 hour before" for
   `reminder.hour_before`);
5. **a translation written for this catalog**, in `scripts/authored-translations.json`;
6. English.

iOS's printf placeholders (`%d`, `%2$@`) are read **by their index** into the key's declared
arguments, so a translation that reorders them keeps them straight. A value iOS has left in
English never outranks a real translation lower down.

`provenance.json` records which of three kinds each key's translations are, **from where its
values actually came** rather than from which files mention it:

- **harvested**: every language rests on a shipped catalog (web, iOS or Windows);
- **authored**: at least one language still rests on `scripts/authored-translations.json`.
  **These want a native speaker's pass.** They are kept as a committed input rather than only in
  the output, so a correction survives a re-run of the seeder. A re-run **prunes** each authored
  value a shipped catalog now supplies, so that file holds only words that ship;
- **englishOnly**: no translation at all. Nothing is in this state today.

Tranche 2 (AITD-660) added iOS as a source, which took the authored keys from 62 to 32. Tranche 3
(AITD-662) hoisted the strings iOS and web share into the shared section; see *The shared section*.

## Adoption

**No client reads `generated/` yet.** This repository is the single source; switching each client
over is its own task on its own board, because each means retiring that client's hand-maintained
catalog and re-pointing its lookups — and web's half also retires `PRODUCT_CONTRACT.md` §2.

**There are two ways in, and the cheaper one already works.**

- **Read `generated/`.** The full switch: the client's lookups read this catalog's output and its
  hand-maintained one is retired. Nobody has taken this path yet.
- **Harvest the words and gate on the key list.** astrid-windows did this first (task `b63d8679`,
  2026-11-14). It copied the words it was missing from `strings/` into its own `Strings.cs` and
  `.resw`, and added `crates/astrid-ffi/tests/copy_catalog_contract.rs`, which holds that shell's
  words against `astrid_core::copy_keys()`. A key the core starts to emit then fails that client's
  build until somebody says what it says, or registers why the client does not draw it. It keeps
  the client's own catalog and file format, ends the drawn-on-screen key at once (D60 is the
  worked example), and costs one test. astrid-ios, astrid-android and astrid-web can take the same
  path before, or instead of, the full switch.
