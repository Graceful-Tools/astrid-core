# Cross-platform contract fixtures

The JSON in `fixtures/` is **generated** from the canonical astrid-web sources by
`export-from-web.mjs`. Never hand-edit it — a rule that is retyped is a rule that drifts, which is
the failure this whole mechanism exists to prevent.

```bash
node contracts/export-from-web.mjs            # regenerate
node contracts/export-from-web.mjs --check    # fail if stale (what CI and predeploy run)
node contracts/export-from-web.mjs --web ../astrid-web
```

The Rust tests compile these files in with `include_str!`, so a stale fixture fails the test suite
rather than going unnoticed at runtime.

## What is covered so far

| Fixture | Canonical source | Consumed by |
|---|---|---|
| `shortcuts.json` | `hooks/useKeyboardShortcuts.ts` — the `KEYBOARD_SHORTCUTS` table plus the `if (selectedTask)` guard read from the dispatch switch | `astrid_core::keyboard` |
| `repeating.json` | `types/repeating.ts` — **executed**, not parsed: every case is run through web's own calculator and the results recorded; `completions` (43) and `zonedProgressions` (4) are whole `nextOccurrence` requests with the person's zone, what astrid-web's server sends (AWTD-1063) | `astrid_core::repeating`, and the rules door's `nextOccurrence` |
| `permissions.json` | `lib/list-permissions.ts` — **executed**: a case matrix run through web's own rules — including the server-only project, status-list and legacy-array branches — recording all eight predicates per case | `astrid_core::permissions` |
| `board.json` | `lib/project-status.ts` — **executed**: three board configurations by eight cards, recording which column each card is in, what every move writes, and what a new card carries | `astrid_core::board` |
| `statuses.json` | `lib/project-custom-states.ts` — **executed**: add, rename, reorder and remove over four boards, recording the role each add mints, every refusal and its message, and the exact array stored afterwards | `astrid_core::board` (the writers) |
| `editing.json` | `lib/editing-session.ts` — **executed**: eleven scripted sequences of begin/end/cancel/commitAll, recording after each step which editor is open, what to commit and what to revert (PRODUCT_CONTRACT.md §6) | `astrid_core::editing` |
| `search.json` | `lib/search-query-parser.ts` — **executed**: thirty-six queries covering every alias, the quoting rule, the identifier shape and the unknown-key fallback, recording the parse and whether it asks for anything | `astrid_core::parse::search`, and the rules door's `searchParse`, which astrid-web's server parses through (AWTD-1062) |
| `task-identifiers.json` | `tests/fixtures/task-identifiers.json` — **copied**, not executed: the server is the only minter of `AWTD-1007`, so there is no client-side arithmetic to run, only the cases every client must parse, autolink and show alike (`docs/specs/TASK_IDENTIFIERS.md`) | `astrid_core::rows::identifier` (the show-rule; the parse and autolink halves are not ported yet) |
| `smart.json` | `lib/task-manager-utils.ts` (`parseTaskInput`) and `lib/i18n/nlp-keywords.ts` — **executed** under a pinned clock: 220 inputs across twelve languages, recording title, lists, due day, priority, repeat and weekdays; the keyword tables themselves ride in the same file so the client reads the same words | `astrid_core::parse::smart` |
| `filters.json` | `lib/date-filter-utils.ts`, `lib/recently-completed-window.ts`, `lib/task-sort.ts`, as `hooks/useFilterState.ts` calls them — **executed** under a pinned clock: ten due-date filters by 28 tasks, the four completion modes over every window kind at three clocks, and eleven sort orders; 569 cases, 127 disputed (D7, D33–D36) | `astrid_core::filters` |
| `manual-order.json` | `lib/list-manual-order.ts` (`sanitizeManualOrder`) and the "manual" sort of `lib/task-sort.ts` — **executed**: 11 reconciliations and 5 arrangements drawn; 14 cases, 2 disputed (D33) | `astrid_core::manual_order`, `filters::sort_by_setting` |
| `reminders.json` | `lib/reminder-snooze.ts`, **executed** against an in-memory reminder queue under a moving clock, plus the v1 snooze route's accepted range (read from its schema): 9 cases, 1 disputed (D37) | `astrid_core::reminders` |
| `markdown.json` | `lib/markdown.ts` (`renderMarkdownWithLinks`) — **executed** in a jsdom window, so through DOMPurify as a page is, and the HTML read back into the core's blocks (the driver documents how): 130 texts: GFM, breaks, links and autolinks, references, task ids, HTML, and 23 `xss-*` sanitisation cases. 130 cases, none disputed: web adopted D38 (AWTD-1064) | `astrid_core::markdown` |

## Two kinds of export

Some contracts are **tables**, and the exporter reads them out of the source. Others are
**arithmetic**, and the only honest way to lock those is to run the canonical implementation and
record what it returns — `drivers/repeating.mjs` imports `types/repeating.ts` and executes it. Node
runs the TypeScript directly, so there is no build step. Only one driver needs astrid-web's
packages: `markdown.mjs` runs web's real renderer (`marked`, DOMPurify) in a jsdom window, so the
checkout needs `npm ci` first (CI runs it with `--ignore-scripts`).

Modules beyond `types/repeating.ts` import astrid-web's `@/…` alias, which Node cannot resolve on
its own. `drivers/alias-loader.mjs` installs a resolve hook that maps it onto the checkout, rather
than requiring a Next.js build to read four pure functions. Two modules are stubbed — the logger
(pino and its transports) and prisma (which opens a database connection at import). The prisma stub
throws on every access, so a driver that ever did reach the database would fail loudly instead of
quietly exporting a fixture built from nulls. Server plumbing imported beside a pure function is
stubbed the same way, throwing on use: `@/lib/redis`, `@/lib/sse-utils` (which starts timers at
import and would hang the export) and the `@prisma/client` package. One driver swaps a stub for a
working fake: `reminders.mjs` runs `lib/reminder-snooze.ts` against an in-memory reminder queue
(`stubs/prisma-reminder-queue.mjs`) that answers only the two calls the rule makes.

A driver runs with `TZ=UTC`. Web's custom repeat path used local date methods, so its results
depended on the machine's timezone (`docs/CONTRACTS.md` D4, closed by AWTD-1063: the zone is now in
the request). The pin stays for the other drivers, which still read the machine's clock.

Still planned: all-day date handling, wire shapes, the task leading control, and the My Tasks scope
(web's lives inside the `useFilterState` hook and cannot be run; D25 records the difference).

## Disputed cases

Where the clients disagree, the core follows iOS (`docs/CONTRACTS.md`, top) — so some cases a
driver runs are ones the core is *meant* to answer differently from web. They are not dropped:
`drivers/disputed.mjs` routes them to the fixture's `disputed` array, each carrying web's answer
and the CONTRACTS.md entry that explains it, and the fixture's `disputes` lists the entries.

- A dispute is a predicate over a case's **inputs** ("an unrecognised due-date filter on an
  undated task is D7's"), never over web's answer, so it cannot hide an unrelated drift.
- A dispute that matches no case fails the export.
- The Rust test requires every `cases` entry to match and, for `disputed`, that **each entry
  still disagrees with web on at least one case** (`crates/astrid-rules/tests/common`). When web
  adopts iOS's behaviour the test fails and the dispute comes out of the driver — an exclusion
  cannot outlive its reason. Where it is cheap, the test also asserts iOS's answer for the
  disputed cases, so that half is locked too.

Older drivers left such cases out of their tables instead (D13's inputs in `smart.json`); those are
inputs no client can reach or a stub no client ports, rather than disputes. `permissions.json` used
to leave out the project-membership and legacy-array branches the same way; it locks them now,
because astrid-web's server decides through this crate with those fields loaded (AWTD-1061).

## Changing a contract

A contract change is a cross-repo change, always in this order:

1. Change the canonical implementation in astrid-web, with its tests.
2. Regenerate these fixtures.
3. Update this crate until its tests pass again, then the clients that pin it.
4. Bump astrid-ios's pinned revision (`core/Cargo.toml`); mirror only what Swift still owns.

Deploy web first — the wire is the one thing every client shares.

## Where this script belongs

It moves into astrid-web as `scripts/export-contract-fixtures.ts` so the canonical repo owns the
export and every client consumes the same artifacts (plan §5.3). The output format will not change
when it does. It lives here for now so the clients are not blocked on that work.
