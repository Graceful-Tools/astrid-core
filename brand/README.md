# The brand profile

`schema.json` is the one description of a brand, and `cargo xtask gen-brand` writes every client's
brand resources from a profile that matches it.

```bash
cargo xtask gen-brand ../astrid-web/brands/acme.brand.json --out /tmp/acme
```

Before this there were four formats and no schema: web's `brands/*.brand.json` carried an `env` bag
of 26 `NEXT_PUBLIC_*` keys validated in TypeScript, iOS read that profile plus an `ios` block mapped
by hand in Swift, Windows kept a **fork** of the same file with five of the keys, and Android used
`brand.properties`, a different format with fourteen values re-typed. A partner needed roughly
seventeen steps on web, fourteen on Apple, ten on Windows and nine on Android — several of them
edits to source.

Where the schema lives was decided by Jon on 2026-11-12: here, because every client already depends
on this repository and the token generator is already in it. Partners keep their profiles wherever
they keep them; web's `brands/` holds Astrid's own and the test fixtures.

## Sections, and the `env` alias

A field lives in a section — `identity`, `colors`, `font`, `assets`, `copy`, `auth`, `capabilities`,
and one block per platform under `platforms`. Each field also carries the `env` key it is **also**
readable as:

```json
"domain": { "type": "domain", "required": true, "env": "NEXT_PUBLIC_BRAND_DOMAIN" }
```

That alias is what lets this land without a migration. Every profile in astrid-web's `brands/`
validates today, unchanged, and a client moves to the sections when it moves rather than everything
moving at once or nothing moving at all. **A section wins over its alias**, so a half-migrated
profile is not quietly overridden by the key it has moved away from.

An empty string is how web's profiles spell "unset", so an empty optional is treated as absent
rather than invalid — otherwise every existing profile fails on its unset keys.

## The `alias`, for a platform block

`env` aliases a field to a flat environment key. A platform block needs the other kind: the key the
**client** spells inside the block, which is not the key the schema spells. That is `alias`:

```json
"package-identity": { "type": "string", "alias": "identityName" }
```

It was not optional. The schema named the platform fields in kebab case and with different *words*
from what each client reads out of the same file — `ios.bundle-id` against astrid-ios's `bundleId`,
`windows.package-identity` against astrid-windows's `identityName` — so `gen-brand` read **none** of
a real profile's platform blocks and wrote every platform file without the partner's identity in it.
Both halves now read the same file (task b8f019d8). A field may list several (`"alias": ["a", "b"]`).

The precedence is one rule all the way down: **a field's own name, then an alias, then `env`, then
the default.** A profile that has migrated says so by carrying the schema's name, and the key it
moved away from never silently wins.

Three keys are deliberately *not* aliased: `android.urlScheme`, `android.accentColor` and
`code.urlProtocol`. They are not another spelling of a platform field — they would be a per-platform
**override** of the shared `identity.url-scheme` and `colors.accent`, which is a design decision
rather than a rename. Every platform output already takes those two from the shared sections.

## What it writes

| Path | For |
|---|---|
| `web/.env.brand`, `web/brand.css` | astrid-web: the env keys it already reads, and the brand custom properties |
| `apple/Brand.xcconfig`, `apple/BrandInfo.plist` | astrid-ios (iOS and Mac): build settings, the URL scheme |
| `android/brand.properties`, `android/colors.xml` | astrid-android |
| `windows/Brand.xaml`, `windows/brand.manifest.json` | astrid-windows: a `ResourceDictionary` and the manifest values |
| `vscode/package.brand.json` | the VS Code extension's identity |
| `code/product.patch.json` | Astrid Code's `product.json`, with ids **derived from the bundle id** |

Astrid Code's ids are a hash of the bundle id and the id's own purpose rather than fresh GUIDs,
because a regenerated id orphans every installed copy's settings. Same profile, same id, on any
machine.

## What it does not write

**Icons.** The task that asked for this ("one 1024-px icon … the icon in all sizes") would need a
rasteriser, and that is a dependency and a `cargo deny` decision rather than another emitter. It is
also against the grain of [`../design/assets`](../design/assets): `crates/xtask/src/assets.rs` exists
*because* the checkbox used to be sixteen fixed-colour PNGs copied by hand, and fixed-colour bitmaps
cannot take the priority tokens or a partner's brand (register DS21). Filed separately.

## Colours a profile does not have to state

`colors.accent-hover`, `accent-ink`, `accent-ink-dark` and `accent-text` are marked
`"derived": true`: state them and they are used, leave them out and
[`astrid_rules::brand_colors`](../crates/astrid-rules/src/brand_colors.rs) derives them from the
accent. One rule, so a partner's brand looks the same on every client — before this each client
derived them differently and iOS fell back to **Astrid's blue**, so a purple brand had a blue hover
on one platform out of four.

**A stated value always wins.** Astrid's own hover and inks are hand-picked on a designed ramp that
no formula reproduces (`#2563EB` → `#1D4ED8` is the next step on it, not a multiply), and they
reach a profile as defaults from `design/tokens.json`'s `brand` block rather than being derived.

**An accent that cannot carry a label is refused**, with the contrast in the message. `accent-text`
is white in every theme (DS26), so an accent white cannot be read on is not a brand accent. That
check existed for Astrid's committed values and never ran on a partner's, which is the hole this
closes — and the first thing it did was fail a shipped fixture: `acme.brand.json`'s `#a855f7` is
3.96:1 and needs to be darker.

## Astrid's own defaults

`design/tokens.json`'s `brand` block is the home of Astrid's accent, hover, inks and label colour.
Three other copies still disagree with it and are each a client's adoption task: astrid-web's
`lib/brand/config.ts`, astrid-ios's `Brand.swift` and astrid-android's README all still say
`#3b82f6`, where the tokens have said `#2563EB` since DS26.
