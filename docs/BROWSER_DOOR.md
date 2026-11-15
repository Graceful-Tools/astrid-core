# The browser's door: which rules the `web/` wasm package answers

*Design note for step 2 of AWTD2-162 (Jon, 2026-11-14: "feature-gate the wire models down to the
rules the browser calls; write up the `Rule` split first, since three clients read that door").
Status: **proposed**. Slice 1 has landed (below). The gate itself waits on the rule set in §4.*

## 1. What the door is, and who reads it

`astrid_rules::rules::run_json` takes one JSON request, `{"kind": "<rule>", …fields}`, and returns one
`{ok, value?, error?}` envelope. It is the same function in three places:

| reader | build | features |
|---|---|---|
| iOS, macOS, Android | `astrid-uniffi` → `astrid-core` → `astrid-rules` | default (everything) |
| Windows | `astrid-ffi` (in astrid-windows) → `astrid-core` | default (everything) |
| astrid-web's **server** | `scripts/build-wasm.sh` → `node/` | `astrid-rules-wasm/markdown` (everything) |
| astrid-web's **browser** (not yet loaded) | `scripts/build-wasm.sh` → `web/` | no markdown; **this note** |

Only the last row changes. The browser is the one reader that pays for every byte at page load,
and the 150,000-byte brotli target in the README is for that row alone.

## 2. What the bytes are. Measured, not the assumption the task started with

All on the pre-`wasm-bindgen` browser artifact, brotli at quality 11 (`scripts/wasm-size.mjs`),
macOS, after slice 1 (260,034 bytes).

**The internal tag is not the cost.** The earlier hypothesis was that `#[serde(tag = "kind")]` makes
serde buffer every request into `Content` and so doubles the decoders. Rebuilt externally tagged,
which is wire-incompatible and was measured only, the code section drops 980 KB → 842 KB raw but
only **−3,959 brotli bytes**. The per-type decoding moves from `serde::private::de::content` into
`serde_json::de`. It does not disappear. So keeping the wire format costs about 4 KB. That is not
worth a door change for three clients, and **this note keeps `{"kind": …}` as it is**.

**The doubled decoders were real, and they came from two constructors, not from the tag.**
`TaskList::new` and `Rule::NextOccurrence` each built a value with `serde_json::from_value`. That
compiled a second complete `TaskList`/`Task` decoder over `serde_json::Value`, 85,681 bytes of
code. Removing either one alone saves nothing (271,374 and 271,185), because each keeps the other's
copy alive. Removing both saves 11,151. That is slice 1.

**What remains is how many rules are compiled.** There are 47. Gating variants out (an
experiment with `#[cfg(any())]` on the variant and its arm) gives:

| browser build answers | brotli | raw |
|---|---:|---:|
| all 47 rules (today, after slice 1) | 260,034 | 1,962,151 |
| all but `completion` and `nextOccurrence` | 198,214 | 1,013,917 |
| a 20-rule browser set (§4, candidate A) | **169,050** | 796,092 |
| the 12 of those that take no `Task` | 126,212 | 551,856 |
| `smartParse` alone (the floor: serde_json, chrono, the keyword tables) | 104,543 | 424,715 |

`completion` and `nextOccurrence` are the only rules that parse an IANA zone name
(`rules::zone`). Leaving them out lets the linker drop chrono-tz's zone table, 900 KB raw, with
**no API change**. That bears directly on step 3 (§5).

Marginal cost of each rule in candidate A, in brotli bytes removed when it alone is dropped:

| rule | bytes | rule | bytes |
|---|---:|---|---:|
| `rows` | 20,410 | `boardRows` | 1,417 |
| `smartParse` | 11,257 | `boardRow` | 1,362 |
| `listAccess` | 3,477 | `searchParse` | 1,305 |
| `identifierLinks` | 3,210 | `tradeoffGrid` | 1,292 |
| `palette` | 3,079 | `ratingKey` | 1,188 |
| `decision` | 2,406 | `boardMove` | 1,070 |
| `board` | 1,645 | `boardPresets` | 912 |
| `paletteRank` | 1,574 | `identifierFormat`, `reminderSnooze`, `manualOrder`, `identifierParse`, `theme` | 370–689 each |

These overlap, since rules share `Task`, `TaskList` and filters, so the sum (about 58 KB) is more
than dropping them all would save.

## 3. The mechanism

1. **One cargo feature per rule group on `astrid-rules`, all in `default`.** For example
   `rules-repeating`, `rules-waiting`, `rules-shell` (see §4). Each gated variant and its arm in
   `run` carry `#[cfg(feature = "…")]`. Native clients and the Node package build with defaults,
   so they compile exactly what they compile today. That is checked the same way the markdown
   gate was: `generated/schema/rule.json` must not change, and `cargo test --workspace` stays green.
2. **`astrid-rules-wasm` names the browser's groups explicitly** (`default-features = false,
   features = [...]`). Its own `full` feature, used for `node/`, turns all of them back on, which
   replaces today's `markdown` forwarding.
3. **A rule the browser build does not carry is refused, not mis-answered**: the existing
   `badRequest` envelope, `unknown variant `<kind>``. This is what a client sees today for a kind
   its pinned core predates, so web's loader needs no new failure path. It falls back to its
   TypeScript copy or asks the server, as it already must while the module loads.
4. **The browser package says what it answers.** Add a `kinds()` export next to `coreVersion()`
   that lists the compiled kinds, so web can choose per call rather than discover a refusal.
   `scripts/check-wasm-packages.mjs` asserts the list for each package, and CI runs it.
5. **Tests.** Every rule test that goes through `run_json` for a gated kind gets the same `cfg`.
   CI already runs `cargo test -p astrid-rules --no-default-features` for the markdown gate, and it
   keeps doing so with the browser's feature set instead.

No variant is renamed, moved or re-typed, so no reader's request changes shape.

## 4. The decision this note needs: which rules does the browser answer?

From the two web tasks this unblocks: "Run core rules in the browser" (AWTD-1586: smart parse,
markdown, permission affordances, filters and sort) and "Adopt the core's rows door" (AWTD-1663:
`rows`, `manualOrder`, `reminderSnooze`, `palette`, `identifierLinks`, `theme`).

**Candidate A, 20 rules, 169,050 bytes:** `smartParse`, `searchParse`, `listAccess`, `board`,
`boardRows`, `boardRow`, `boardMove`, `boardPresets`, `rows`, `manualOrder`, `reminderSnooze`,
`palette`, `paletteRank`, `identifierLinks`, `identifierParse`, `identifierFormat`, `theme`,
`decision`, `tradeoffGrid`, `ratingKey`.

Left out, and why that seems safe (each one is a question for web, not a fact):

- **`completion`, `nextOccurrence`, `repeatSummary`**: web completes a repeating task on the
  server. Leaving these out also takes chrono-tz out (§2).
- **`waitingWrite`, `waitingWake`, `waitingSettle`, `handBack`**: server-side writes.
- **Shell presentation rules**: `shortcut`, `calloutPlacement`, `detailLayout`, `leadingPicker`,
  `listChips`, `dayOffset`, `quickAddDefaults`, `reminderTriage`, `mentionTrigger`,
  `mentionInsert`, `copyOnly`, `commentVisibility`, `capabilities`, `listDefaultPicks`,
  `nesting`, `pluralCategory`, `customAgentName`, `brandColors`. These exist for the native
  shells. Web has its own components for each, and any one can join the set later at its
  marginal cost.
- **`listProjectMoves`**: the README says web's client asks it, so it may belong in A. Not
  measured separately yet.

**Candidate A is still 19 KB over 150,000.** `rows` alone is 20 KB, and it is the rule AWTD-1663
exists for. Ways to close the gap, cheapest first:

1. Find out why `rows` costs 20 KB. It pulls the filters, sort, `User` and `my_tasks`
   preferences, and their decoders are probably most of it. A first look is the next slice
   either way.
2. Drop `paletteRank` if `palette` serves web's palette, and `decision`/`tradeoffGrid`/
   `ratingKey` if web keeps them in TypeScript (their marginal costs are in §2).
3. Accept a browser target in **gzip**, which is how AWTD-1586 states its acceptance
   ("≤150 KB gzipped"), rather than brotli. That is Jon's call, not this note's.

## 5. What this does to step 3 (Intl offsets instead of chrono-tz)

If the browser does not answer `completion` and `nextOccurrence`, chrono-tz's zone table is
already out of the browser build, and step 3's API change (the caller passes offsets from `Intl`)
buys the browser nothing. Step 3 is then only needed if the browser must run repeating rollover
itself. **Recommendation: decide §4 first. Do step 3 only if the answer puts repeating rules in
the browser.**

## 6. Slices

1. ~~Take out the two `from_value` constructors~~. **Done**: 271,185 → 260,034. No wire or API
   change; equivalence tests in `model/list.rs` and `rules.rs`.
2. The feature groups and `kinds()` (§3), with every group still on for the browser. This is the
   mechanism only, so its size change is about zero.
3. Turn the browser's set down to what §4 decides, and ratchet the budget.
4. `rows` (§4.1), then whatever else the 150,000 still needs.
