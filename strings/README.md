# `strings/` — one catalog, four platforms

**The words for every key this core asks a client to say**, in the twelve languages the product
ships, generated into web's, Apple's, Android's and Windows' own formats.

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
| `generated/` | `cargo xtask gen-strings` output. **Do not edit**; `check-strings` regenerates and compares |

`cargo xtask check-strings` is the gate, and it is in CI. It fails when a key this core emits is
not in the catalog, when a key in the catalog is no longer emitted, when a language is missing
words, when a translation interpolates an argument the schema does not declare — and when
`generated/` is not what the source says.

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

### Two keys for one word

The repeat **filter** asks for `repeat.daily`; the repeat **picker** asks for `repeating.daily`.
Both are in the catalog, because the catalog's job is to make every key the core emits sayable.
`Strings.cs` carries only the second, so astrid-windows' filter sheet draws the literal text
`repeat.daily` today — recorded in `docs/CONTRACTS.md`, and making the core emit one spelling is a
change to a pinned contract rather than a catalog edit.

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

**No key declares one yet.** The four `repeat.every_n_*` keys are the candidates — today both iOS
and Windows special-case a count of one in the shell, with a separate `repeat.every_day` string.
Their per-language categories cannot be harvested from catalogs that do not have them, and a
Russian `few` form is a translator's to write. The support is here so that filling them in is a
change to the data rather than to the format; `crates/xtask/src/strings.rs` exercises all four code
paths against a fixture.

## Translations

`scripts/seed-strings.mjs` wrote the first version of this catalog, and can be re-run. It joins
what already exists rather than inventing anything: the core key's English (astrid-windows'
`Strings.cs` fallback table, the only place it was written down), then the web key that says the
same English, then that key's twelve translations.

`provenance.json` records which of three kinds each key's translations are:

- **harvested** — from astrid-web's twelve locales, or from astrid-windows' German `.resw`, which
  is a translation of these very keys;
- **authored** — written for this catalog, in `scripts/authored-translations.json`, because no
  reachable catalog carries the key. **These want a native speaker's pass.** They are kept as a
  committed input rather than only in the output, so a correction survives a re-run of the seeder;
- **englishOnly** — neither. Nothing is in this state today.

iOS's `Localizable.strings` is **not** a source yet: it is 1,421 keys in twelve languages and by
far the largest body of translation the product has, but it lives in astrid-ios. Harvesting the
420 strings it shares with web is the next tranche, and it belongs to a task on that board.

## Adoption

**No client reads `generated/` yet.** This repository is the single source; switching each client
over is its own task on its own board, because each means retiring that client's hand-maintained
catalog and re-pointing its lookups — and web's half also retires `PRODUCT_CONTRACT.md` §2.
