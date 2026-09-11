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
| `repeating.json` | `types/repeating.ts` — **executed**, not parsed: every case is run through web's own calculator and the results recorded | `astrid_core::repeating` |
| `permissions.json` | `lib/list-permissions.ts` — **executed**: a case matrix run through web's own rules, recording all eight predicates per case | `astrid_core::permissions` |
| `board.json` | `lib/project-status.ts` — **executed**: three board configurations by eight cards, recording which column each card is in, what every move writes, and what a new card carries | `astrid_core::board` |
| `statuses.json` | `lib/project-custom-states.ts` — **executed**: add, rename, reorder and remove over four boards, recording the role each add mints, every refusal and its message, and the exact array stored afterwards | `astrid_core::board` (the writers) |
| `editing.json` | `lib/editing-session.ts` — **executed**: eleven scripted sequences of begin/end/cancel/commitAll, recording after each step which editor is open, what to commit and what to revert (PRODUCT_CONTRACT.md §6) | `astrid_core::editing` |
| `search.json` | `lib/search-query-parser.ts` — **executed**: thirty-six queries covering every alias, the quoting rule, the identifier shape and the unknown-key fallback, recording the parse and whether it asks for anything | `astrid_core::parse::search` |
| `smart.json` | `lib/task-manager-utils.ts` (`parseTaskInput`) and `lib/i18n/nlp-keywords.ts` — **executed** under a pinned clock: 220 inputs across twelve languages, recording title, lists, due day, priority, repeat and weekdays; the keyword tables themselves ride in the same file so the client reads the same words | `astrid_core::parse::smart` |

## Two kinds of export

Some contracts are **tables**, and the exporter reads them out of the source. Others are
**arithmetic**, and the only honest way to lock those is to run the canonical implementation and
record what it returns — `drivers/repeating.mjs` imports `types/repeating.ts` and executes it. Node
runs the TypeScript directly, so no build step and none of astrid-web's dependencies are involved.

Modules beyond `types/repeating.ts` import astrid-web's `@/…` alias, which Node cannot resolve on
its own. `drivers/alias-loader.mjs` installs a resolve hook that maps it onto the checkout, rather
than requiring a Next.js build to read four pure functions. Two modules are stubbed — the logger
(pino and its transports) and prisma (which opens a database connection at import). The prisma stub
throws on every access, so a driver that ever did reach the database would fail loudly instead of
quietly exporting a fixture built from nulls.

A driver runs with `TZ=UTC`. Web's custom repeat path uses local date methods, so its results depend
on the machine's timezone (see `docs/CONTRACTS.md` D4); without pinning, the fixture would record
whichever zone the person generating it happened to be in.

Planned, as each module is ported (plan §3): repeating-task rollover, the list permission matrix,
all-day date handling, the smart-task parser, wire shapes, the task leading control, and the
editing-session machine.

## Changing a contract

A contract change is a cross-repo change, always in this order:

1. Change the canonical implementation in astrid-web, with its tests.
2. Regenerate these fixtures.
3. Update this client until its tests pass again.
4. Mirror it into astrid-ios (iOS and Mac share that code).

Deploy web first — the wire is the one thing every client shares.

## Where this script belongs

It moves into astrid-web as `scripts/export-contract-fixtures.ts` so the canonical repo owns the
export and every client consumes the same artifacts (plan §5.3). The output format will not change
when it does. It lives here for now so this client is not blocked on that work.
