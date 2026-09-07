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

## Two kinds of export

Some contracts are **tables**, and the exporter reads them out of the source. Others are
**arithmetic**, and the only honest way to lock those is to run the canonical implementation and
record what it returns — `drivers/repeating.mjs` imports `types/repeating.ts` and executes it. Node
runs the TypeScript directly, so no build step and none of astrid-web's dependencies are involved.

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
