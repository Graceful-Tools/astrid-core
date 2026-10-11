# Cross-platform contract fixtures

The JSON in `fixtures/` is **generated in astrid-web**, by its
`scripts/export-contract-fixtures.ts`, and copied here. Never hand-edit it — a rule that is retyped
is a rule that drifts, which is the failure this whole mechanism exists to prevent. The files here
are copies: to change one, change the rule in astrid-web and regenerate there.

```bash
# in astrid-web
npm run export:contract-fixtures             # regenerate
npm run check:contract-fixtures              # fail if stale (its predeploy runs this)

# here
cargo xtask check-contracts [../astrid-web]  # fail unless our copies are byte-for-byte web's
```

The exporter used to exist **twice** — once there and once here as `export-from-web.mjs` — and the
two copies drifted: `permissions.json` alone differed by 1800 lines, and this repo's CI was
checking core against the retired generator. There is one exporter now, in astrid-web, and the
check here is a comparison rather than a second generation (AWTD-1186).

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
record what it returns. The drivers that do that now live in astrid-web, under
`scripts/contract-fixtures/drivers/`, and that README section is theirs; the short version is that
each driver imports astrid-web's own module and executes it, with a resolve hook for the `@/…`
alias, stubs for the logger, prisma and the server plumbing imported beside a pure function, and
`TZ=UTC` pinned so a fixture never encodes whoever ran it.

Still planned: all-day date handling, wire shapes, the task leading control, and the My Tasks scope
(web's lives inside the `useFilterState` hook and cannot be run; D25 records the difference).

## Disputed cases

Where the clients disagree, the core follows iOS (`docs/CONTRACTS.md`, top) — so some cases a
driver runs are ones the core is *meant* to answer differently from web. They are not dropped:
astrid-web's `scripts/contract-fixtures/drivers/disputed.mjs` routes them to the fixture's
`disputed` array, each carrying web's answer and the CONTRACTS.md entry that explains it, and the
fixture's `disputes` lists the entries.

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
2. Regenerate the fixtures there, and copy them here.
3. Update this crate until its tests pass again, then the clients that pin it.
4. Bump astrid-ios's pinned revision (`core/Cargo.toml`); mirror only what Swift still owns.

Deploy web first — the wire is the one thing every client shares.

## Where this script lives

In astrid-web, as `scripts/export-contract-fixtures.ts` — the canonical repo owns the export, so a
change to a shared rule fails `--check` in the pull request that made it rather than as a red build
here afterwards (plan §5.3). The move finished in AWTD-1186; the output format did not change.
