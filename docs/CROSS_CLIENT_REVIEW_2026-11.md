# Cross-client review — November 2026

*How much of Astrid lives in the core, how each client themes and brands itself, and what it would take for white-labelling to be one command per platform.*

**Written for:** Jon, and the engineers and agents who pick up the tasks it files.
**Scope:** astrid-core, astrid-ios (iOS and Mac), astrid-web as a client and as the brand hub, astrid-android, astrid-windows, astrid-vscode and astrid-code. The server's scaling and hosting were covered by astrid-web [`docs/architecture/PLATFORM_ASSESSMENT_2026-11.md`](https://github.com/Graceful-Tools/astrid-web/blob/main/docs/architecture/PLATFORM_ASSESSMENT_2026-11.md) (2026-11-01). This review does not repeat it.
**Snapshot:** six read-only audits on 2026-11-11. Revisions read: core `af91499`, iOS `063ef1b2`, web `760cd4fb`, Android `137dc6a`, Windows `948bdf9`, astrid-vscode `1ab2ae3`, astrid-code `57af183`. Every `file:line` below refers to those revisions. They describe that day, not whatever state you are reading this in.

The tasks filed from this review are listed in [§6](#6-tasks-filed). Six of them need a decision from Jon first; those are collected in **AITD-626**.

---

## 1. Headline

The architecture is the right shape. The core is pure where it should be, the contracts are fixture-locked, and the native clients carry little business logic. Three things stand between today and "white-label in one command":

1. **There are four brand formats, and no client can be branded from one file.**
   - Web reads `brands/*.brand.json`.
   - iOS reads that file plus an `ios` block.
   - Windows keeps a *forked* copy of the file.
   - Android has its own `brand.properties`.

   No schema exists. A partner's accent is never contrast-checked, and every client derives hover and ink colours differently, so Acme's purple gets a **blue** hover on iOS. Counting the manual steps per platform:

   | Platform | Manual steps | Of which edit source |
   |---|---|---|
   | Web (server + client) | about 17 | |
   | iOS + Mac | about 14 | 4 |
   | Windows | about 10 | 3 |
   | Android | about 9 | |
   | VS Code extension | about 12 | |
   | Astrid Code | about 8 | |

2. **Web and VS Code can reach only 10 of the core's rules.**
   - The stateless door (`astrid-rules`, built to wasm) exposes `completion`, `nextOccurrence`, `listAccess`, `smartParse`, `searchParse`, `renderMarkdown`, `repeatSummary`, `customAgentName`, `board` and `boardMove`.
   - Everything else is reachable only through the stateful `App` (203 commands), which needs SQLite and tokio and so can't be built to wasm.
   - The list pipeline, the most-duplicated logic in the system, is only assembled inside `App`, and even there `rowsForList` returns ids only. That covers filters, sort, manual order, My Tasks, due labels, overdue, chips and the detail field order.
   - So web keeps about 3,850 LOC of TypeScript that duplicates fixture-tested core rules, iOS keeps about 420 LOC of row projection, and the extension has an overdue bug the core already fixed.

3. **Every native client ships a known SQLite advisory, and the pins have drifted.**
   - The fix for RUSTSEC-2026-0459 (`69f26d7`, rusqlite 0.41) is in core but in **none** of the native pins.
   - iOS's Core supply chain CI has been red on `main` since 2026-11-09.
   - .NET 9, which the Windows MSIX bundles self-contained, left support on 2026-11-10.

### What "white-label in one command" looks like

One profile, validated in one place, drives every client:

```
brands/acme.brand.json ──► cargo xtask gen-brand ──► web .env.brand + CSS vars
          (schema in core)        │                  Apple xcconfig/plist (scheme, Google client)
                                  │                  Android brand.properties + colors.xml
                                  │                  Windows manifest + Brand.xaml
                                  │                  VS Code package.json identity, colour theme
                                  │                  Astrid Code product.json patch (derived GUIDs)
                                  └─ one 1024-px icon ► every icon size on every platform
```

Each client then has one command: `brand:launch` (web), `make-brand-variant.sh` / `appstore-release.sh --brand` (Apple), `make-brand-variant.sh` (Android), `package.ps1 -Brand` (Windows), `make-brand-extension.mjs` and `make-brand-variant.mjs` (VS Code / Astrid Code). Each ends with a literal audit that fails on any leftover "Astrid".

---

## 2. The core: what it holds, and what clients still re-implement

### 2.1 Pins (measured against `af91499`)

| Consumer | Pinned at | Behind | Recorded in CONSUMERS.md |
|---|---|---|---|
| web `astrid-rules` | `52a371d` | 68 | yes |
| web `astrid-design` | `73e4b02` | 3 | no |
| Windows | `8c3274d` | 26 | yes |
| iOS | `a503a62` | 12 | the row said `3677cfb` (corrected with this review) |
| Android | `2cf2b5f` | 72 | yes |
| astrid-vscode wasm | `3dcc880` | 22 | **no** |
| astrid-code wasm | `765e38e` | 24 | **no** |

All three native consumers float `rust-toolchain = "stable"`, while the core pins 1.98.1. → AWTD2-160, AITD-615, AATD-87, AWTD2-157, AWTD-1585.

### 2.2 Rules duplicated in clients, by size and divergence risk

| Where | What | LOC | Core already has | Task |
|---|---|---|---|---|
| web | sort, date windows, manual order, My Tasks, identifiers, markdown, NLP keywords, snooze, project status… | about 3,850 | `astrid-rules` filters, rows, parse, identifier, reminders | AWTD-1583, AWTD-1586, AWTD2-161 |
| web | server fallbacks that still run on every decision (permissions, search, repeating) | about 1,660 | `listAccess`, `searchParse`, `completion` | AWTD-1591 |
| iOS | identifiers, drag nesting, new-task defaults, triage, capabilities, palette, keyboard | about 1,000 | yes | AITD-621 |
| iOS | row leading control, detail field order, chips, day offset | about 420 | `rows::TaskRow`, `rows/detail.rs`, but not exposed | AWTD2-161 → AITD-622 |
| iOS | about 40 `AstridAPIClient` calls with an existing core command | part of 2,103 | commands exist | AITD-620 |
| iOS + web | decision cards and trade-off grids | 280 + 231 | **nothing**: `decisions.json` has no reader | AWTD2-167 |
| Android | sidebar filter (**bug**: shows milestones and iterations) | small | `is_domain_list` | AATD-89, AWTD2-166 |
| Windows, iOS, Android | sidebar order; list-defaults tables; English-only repeat sentences | small each | partly | AWTD2-166, AWTD2-169 |
| VS Code | board columns (statusRole stand-in), completion patch, permission fallback, lane names, branch names; overdue **bug** | about 200 | `board`, `complete_as`, `is_overdue` | Astrid Code board |
| every shell | which background loops start (Apple 3 of 5, the others 5) | | | AWTD2-168 |
| every shell | request/response types written by hand (Windows 3,525 LOC, Android 114 DTOs, iOS facade, web) | about 6,000 | only `kind` names are checked | AWTD2-165 |

### 2.3 Why the stateless door matters most

The pure pieces are already in `astrid-rules`. The assembly in `astrid-core/src/app/dispatch/row_json.rs:73-283` is the only thing standing between them and a wasm caller. Move that assembly into `astrid_rules::rows`, then expose these on both doors:

- `Rule::Rows`
- `Rule::TaskRow`
- `Rule::Palette`
- `Rule::IdentifierLinks`
- `Rule::ReminderSnooze`
- `Rule::CompletionPatch`
- `Rule::Theme`

That unblocks web, VS Code and iOS at once (AWTD2-161; coordinate with AWTD2-145).

The wasm itself has to get smaller before the browser will load it:
- 1.85 MB raw, 266 KB brotli, with no `wasm-opt` step and no budget.
- `parse/smart.rs:32` embeds 45 KB of test cases.
- `chrono-tz` is probably most of the 1.08 MB data section.

→ AWTD2-162, target under 150 KB brotli.

---

## 3. Theming

| Client | Source of colours | Themes | Gap | Task |
|---|---|---|---|---|
| Web | vendored `packages/astrid-design` (generated) | light, dark, ocean | hardcoded palette classes and hex, owned by AWTD-1575 | AWTD-1575 |
| iOS / Mac | vendored `AstridTokens.swift` (AITD-605/610/611) | all three | 97 fixed `Theme.Dark.*`/`Theme.Ocean.*` reads and 49 theme-name comparisons that the ratchet doesn't count; dead `LiquidGlass`; `MacLayout` restates token values; the re-vendor is blocked on the missing `Brand.accentInk*` | AITD-623, AITD-615 |
| Android | hand copy of iOS `Theme.swift`, drifted (accent, error, inks, Ocean text, dark focus) | all three | no Compose emitter; 618 literal `.dp`, 75 named colours | AWTD2-163, AATD-85 (comment added), AATD-86 |
| Windows | hand-written from web's *old* CSS: 58 hex in `App.xaml`, accent still `#3B82F6` | Ocean is a background brush, not a theme; HighContrast empty | XAML emitter | AWTD2-155, AWTD2-156 |
| VS Code ext | `--vscode-*` variables only, no hex | follows any editor theme | priority colours from `charts-*`, not tokens | Astrid Code board |
| Astrid Code | none | | no generated colour theme, so a brand accent never reaches the workbench | AWTD2-163 |

The token system itself is in good shape: a separate `brand` block, AA checks, hash-checked vendoring. Two gaps remain:

- **Partner colours.** The partner's accent is never validated, and Astrid's default accent is written down five times with two different values. → AWTD2-159.
- **Missing tokens.** There is no font-family token, no chart palette and no high-contrast theme. These belong in the brand schema (AWTD2-158).

Reduce Motion is ignored everywhere on Apple (DS17). → AITD-624.

---

## 4. White-labelling, client by client

| Client | Already brandable | Still needs a source edit, or can't be done | Task |
|---|---|---|---|
| Web (server) | env/profile identity, OIDC, Docker self-host | one image per brand (`NEXT_PUBLIC_*` baked in); legal pages say "Graceful Tools LLC"; **feature requests default to `jon@gracefultools.com`**; manifest, favicons and service-worker icons ignore `BRAND_ICON`; the profile parser rejects the `windows` block | AWTD-1584, AWTD-1587, AWTD-1588, AWTD-1589, AWTD-1590 |
| iOS / Mac | bundle ids, App Group, keychain group, team, display name (`make-brand-variant.sh`); copy is clean | Google client id hardcoded; `astrid://` literal in 8 files, and the core's `urlScheme` isn't passed; icons and character art copied by hand into two catalogs; Mac Direct entitlements; English-only permission prompts; release script knows only the Astrid schemes | AITD-616, AITD-617, AITD-618, AITD-619, AITD-624 |
| Android | `brand.properties` (14 keys), app links | own format; 15 mipmaps, notification icons and character art by hand; "Loving Astrid?" in 12 locales; feedback subject; Ocean cyan; Play listing; no literal audit | AATD-90 |
| Windows | `package.ps1 -Brand`: MSIX identity, name, scheme, server | accent hardcoded; `Astrid.ico` compiled in; in-app marks; tile generator has no output folder; sign-in copy names Google; export and msix file names; listing | AWTD2-172 |
| VS Code ext | OAuth DCR (no per-partner client) | `package.json` identity, publisher, URI, server default; about 25 TS literals; no localisation; `openLink` allows only `astrid:` | Astrid Code board |
| Astrid Code | | about 25 `product.json` fields including GUIDs and UUIDs; `electron.ts` hardcodes company and copyright; about 30 `localize` strings | Astrid Code board |

### Cross-client localisation

There are four catalogs: web 937 keys, iOS 1,421, Android 848 (ported from iOS), Windows 736 (2 languages). Each uses its own key style.

- 420 iOS strings share English text with web, but only 106 sit under the mapped key.
- The core emits about 49 keys. Exact-name matches by client: Windows 38, iOS 11, web 6, Android 0.
- One catalog in the core, generating every platform's format, retires the hand registry in web `PRODUCT_CONTRACT.md` §2.

→ AWTD2-164, then AWTD-1592, AWTD2-173, AATD-96.

---

## 5. Efficiency and dependencies

| Area | Finding | Task |
|---|---|---|
| Security | SQLite RUSTSEC-2026-0459 in iOS, Mac, Android and Windows; .NET 9 out of support | AITD-615, AATD-87, AWTD2-157, AWTD2-171 |
| Android | **no `abiFilters`**: JNA ships armeabi-v7a etc. with no core library, likely crashing on 32-bit; `.so` 16.3 MB with about 3.5 MB of symbols; every library at its Dec 2024 version; targetSdk 35; core started on the main thread with no baseline profile | AATD-88, AATD-91, AATD-92, AATD-93 |
| Apple | 21 MB iOS / 44 MB Mac binary from the staticlib; no `opt-level="s"`; bundled SQLite duplicates the system's; 161 MB xcframework zip on every Xcode Cloud run | AITD-625, AWTD2-170 |
| Web | about 25 unused or misplaced deps (17 unused Radix packages, two toast systems, three ID generators); first-load JS about 404 KB gzipped on **every** route against a 250 KiB budget, ungated | AWTD-1593 |
| Windows | 12 dead workspace deps; duplicate `windows-sys`; no JSON source generation or ReadyToRun; refresh storms after change events; a committed crash dump | AWTD2-174, AWTD2-175 |
| VS Code | 7–9 requests per SSE burst (full reload, 1000-task fetch); no CI or Node pin; vsce advisories | Astrid Code board |
| Core | Android TLS ignores device and enterprise CAs (breaks partners behind an inspecting proxy); no cargo features for sync providers; four differing `deny.toml` files | AWTD2-170 |

---

## 6. Tasks filed

All tasks were filed in **Inbox**, not Ready, so no loop starts one before Jon promotes it. Priority 3 is the highest. Core tasks live on the Windows board (AWTD2-*), as before.

**Decisions:** AITD-626 (assigned to Jon) covers:
- where the brand schema lives;
- the shared UniFFI crate;
- 32-bit Android;
- the Astrid Code board renderer;
- the `User.locale` migration;
- promoting the three security bumps.

### astrid-core (Windows board)

| Task | P | Summary |
|---|:-:|---|
| AWTD2-158 | 3 | One brand profile schema in core, plus `cargo xtask gen-brand` writing every client's brand resources |
| AWTD2-159 | 3 | Derive hover and ink from one accent; refuse a partner accent that fails contrast |
| AWTD2-160 | 2 | Record every consumer and vendored copy; `check-pins` reads them all; toolchain check |
| AWTD2-161 | 2 | List-row assembly into `astrid-rules`; TaskRow projections from both doors |
| AWTD2-162 | 2 | Shrink the wasm for the browser; CI size budget |
| AWTD2-163 | 2 | Compose and VS Code colour-theme emitters (XAML is AWTD2-155) |
| AWTD2-164 | 2 | One string catalog generating web JSON, Apple strings, Android XML, resw |
| AWTD2-165 | 2 | JSON Schema for commands, rules and responses |
| AWTD2-166 | 2 | `sidebar` command |
| AWTD2-167 | 2 | Decision cards and trade-off grids in `astrid-rules` |
| AWTD2-168 | 1 | One UniFFI crate; background loops and build revision in core (needs Jon) |
| AWTD2-169 | 1 | List-default picks and plural-aware repeat summaries |
| AWTD2-170 | 1 | Android platform CAs; optional bundled SQLite; per-provider features; one `deny.toml` |

### Windows

| Task | P | Summary |
|---|:-:|---|
| AWTD2-171 | 3 | .NET 10 LTS and Windows App SDK 2.x; clear Dependabot |
| AWTD2-172 | 2 | Partner package edits no tracked file |
| AWTD2-173 | 2 | Ten missing languages; registry keys |
| AWTD2-174 | 1 | Coalesce refreshes |
| AWTD2-175 | 1 | Dead deps, JSON source generation, ReadyToRun, stackdump |

Existing tasks this builds on: AWTD2-155, AWTD2-156, AWTD2-157, AWTD2-145, AWTD2-153.

### iOS / Mac

| Task | P | Summary |
|---|:-:|---|
| AITD-615 | 3 | Core bump past the SQLite advisory; ink colours; re-vendor tokens |
| AITD-616 | 2 | Brand URL scheme to the core; the eight `astrid` literals |
| AITD-617 | 2 | Google client id from the profile |
| AITD-618 | 2 | Variant installs icon and art; Mac Direct |
| AITD-619 | 2 | `appstore-release.sh --brand` |
| AITD-620 | 2 | About 40 API calls onto core commands |
| AITD-621 | 1 | Delete the Swift copies of core rules |
| AITD-622 | 1 | Rows from TaskRow (waits on AWTD2-161) |
| AITD-623 | 2 | Ratchet counts fixed-theme picks; LiquidGlass; MacLayout |
| AITD-624 | 1 | Reduce Motion; localised permission prompts |
| AITD-625 | 1 | Shrink the staticlib and the xcframework download |

### Android

| Task | P | Summary |
|---|:-:|---|
| AATD-87 | 3 | Core bump (72 commits, the advisory); pin the toolchain |
| AATD-88 | 3 | `abiFilters` (likely 32-bit crash) |
| AATD-89 | 3 | Sidebar shows milestones and iterations |
| AATD-90 | 3 | One-command brand build from the profile, with a literal audit |
| AATD-91 | 2 | Shrink the `.so`; size budget |
| AATD-92 | 2 | AGP 9, Kotlin 2.4, Compose, targetSdk 36; drop unused deps |
| AATD-93 | 2 | Baseline profile; core off the main thread |
| AATD-94 | 1 | Generated models (waits on AWTD2-165) |
| AATD-95 | 1 | `feedbackListId`; Custom Agent photo |
| AATD-96 | 1 | Drift gate for ported strings; 16 missing |

Existing tasks this builds on: AATD-85 (measurements added as a comment), AATD-86, AATD-19.

### Web

| Task | P | Summary |
|---|:-:|---|
| AWTD-1583 | 3 | Adopt the iOS-resolved list rules (351 disputed cases) |
| AWTD-1584 | 3 | Cross-tenant leaks: legal operator; the feature-request default address |
| AWTD-1585 | 2 | Rules bump (68 behind); board through core |
| AWTD-1586 | 2 | Core rules in the browser (waits on AWTD2-162) |
| AWTD-1587 | 2 | One image, many brands (runtime profile) |
| AWTD-1588 | 2 | Artwork from one source icon |
| AWTD-1589 | 2 | Profile accepts platform blocks; validates against the core schema; core colour derivation |
| AWTD-1590 | 1 | `brand:launch` |
| AWTD-1591 | 1 | Sample the comparison; retire the fallbacks |
| AWTD-1592 | 1 | Locale JSON from the core catalog; localised notifications (`User.locale` needs Jon) |
| AWTD-1593 | 1 | Dependency cleanup; initial-JS gate |

Existing tasks this builds on: AWTD-1575, AWTD-1581, AWTD-1487.

### Astrid Code board (astrid-vscode and astrid-code; this list has no identifiers)

| P | Title |
|:-:|---|
| 3 | [astrid-vscode] Astrid mode's board uses core's board rule — drop the statusRole stand-in |
| 3 | [astrid-vscode] All-day tasks show as overdue on their own due day (west of UTC) |
| 2 | [astrid-vscode] Remove the TypeScript copies of core rules (waits on AWTD2-161) |
| 2 | [astrid-vscode] Vendor the rules wasm with a script and contract-fixture parity tests; flag fork drift |
| 2 | [astrid-vscode] Stop refetching everything on every SSE event |
| 2 | [white-label] Localise the extension and build a branded .vsix from brands/*.brand.json in one command |
| 2 | [astrid-code][white-label] One command produces a branded Astrid Code build from a brand profile |
| 1 | [astrid-code] Type the fork↔extension command contract and choose one board renderer |
| 1 | [astrid-vscode] Hygiene: CI, Node pin, vsce 4, minified bundle, no .map files in the .vsix |

### Suggested order

1. **Security, this week:** AITD-615, AATD-87, AATD-88, AWTD2-157, AWTD2-171, AWTD-1584.
2. **The keystones:**
   - AWTD2-158 and AWTD2-159 (brand schema and colours), which unblock every client's one-command brand build;
   - AWTD2-161 and AWTD2-162 (rows on the stateless door, a smaller wasm), which unblock web and VS Code cutting over to core.
3. **Per-client brand commands:** AITD-616 to AITD-619, AATD-90, AWTD2-172, AWTD-1587 to AWTD-1590, and the Astrid Code white-label pair.
4. **Deleting duplicate logic:** AWTD-1583, AWTD-1586, AITD-620 to AITD-622, and the VS Code rule copies.
5. **Shared catalog and schema:** AWTD2-164 and AWTD2-165 and their adoptions.

---

## 7. Documentation corrected with this review

- **astrid-core:**
  - `CONSUMERS.md`: the iOS row is now `a503a62…`, and a note records the two unrecorded wasm consumers (AWTD2-160).
  - `design/README.md`: Apple *has* adopted the generated Swift; `Brand.accentInk*` doesn't exist yet (AITD-615).
- **astrid-ios:**
  - `docs/CORE_MIGRATION.md`: the pin; the data-layer gaps the core has since closed; what blocks row projections; Decisions stays in Swift.
  - `docs/BRAND_VARIANT.md`: the URL scheme is now iOS work (AITD-616).
- **astrid-web** (in a PR):
  - `docs/WHITELABELING.md`: the AWTD2-95 pin note, the iOS frozen list, icons, the legal operator, per-platform status.
  - `brands/README.md`: the `windows` block claim.
  - `docs/PERFORMANCE_BUDGETS.md`: the 2026-11-01 initial-JS measurement.
  - `PLATFORM_ASSESSMENT_2026-11.md`: a pointer to this review.
- **astrid-android, astrid-windows, astrid-vscode, astrid-code:** the stale statements the audits found (`PARITY` rows, theme source, the vendored-core paragraph, the board known-limit, the `electron.ts` footprint).
