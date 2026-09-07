# Cross-platform contracts, and where the clients disagree

Rules that must behave identically on web, iOS/Mac and Windows. The mechanism is in
[`contracts/README.md`](../contracts/README.md); this file records the **decisions and the known
divergences** — the things a generated fixture cannot tell you.

A divergence listed here is not a Windows bug to fix locally. Fixing one means changing every
client together, and the entry says which behaviour this crate currently follows and why.

---

## 1. Repeating-task rollover

Canonical: `astrid-web/types/repeating.ts` and `astrid-web/lib/repeating-task-handler.ts`.
Apple: `astrid-ios/Astrid App/Utilities/RepeatingTaskHandler.swift`.
Here: `astrid_core::repeating`.

### D1 — Local calendar versus UTC for month and year steps

**This crate follows web.** All arithmetic here is UTC.

Web advances months and years with `setUTCMonth` / `setUTCFullYear`. The Swift port preserves the
time of day through a UTC calendar but then calls `Calendar.current` — the device's local calendar —
for the month and year steps and for the `until_date` comparison.

For a user far enough east or west of UTC, an all-day task stored at UTC midnight sits on a
different local date, so a monthly rollover computed locally can land a day away from what web
computes for the same completion. Two clients then disagree about when the next occurrence is due,
and the last writer wins.

Fixing it means moving Swift onto UTC, with the multi-step progression tests re-run in a non-UTC
timezone on both platforms. Worth a task in the iOS queue; not something to paper over here.

### D2 — Weekly patterns ignore their interval

**Reproduced deliberately.** `astrid_core::repeating::next_weekday_occurrence` ignores `interval`.

Both web and Swift ignore `interval` for weekly custom patterns: `getNextWeekdayOccurrence` takes the
interval and never reads it. So "every 2 weeks on Mon and Wed" advances every week on all three
clients. The stored pattern says one thing and every client does another.

Diverging here would be worse than the bug: this client would schedule occurrences the others do not
have, on a field the user believes is shared. It changes on web first.

### D3 — "Same weekday of the month" drops the time of day

**Reproduced deliberately.**

Both web and Swift build this date from the first of the target month, which is midnight, and then
add days — so a task due at 10am on the third Tuesday rolls over to midnight on the next third
Tuesday. Same reasoning as D2: the fix belongs on web, and until then matching matters more.

`same_date` monthly patterns are unaffected — they keep their time.

### Settled behaviour (no divergence)

- **"Until date" is inclusive and compared by date, not instant.** "Repeat until Dec 15" means an
  occurrence on Dec 15 still runs.
- **"Never" outranks a stale limit.** An end condition of `never` does not terminate even when
  `end_after_occurrences` or `end_until_date` still hold values from an earlier edit.
- **Month-end clamping.** January 31st plus a month is the last day February has, never March 2nd.
- **The due time survives both repeat modes.** Completing a 9am task at 11pm reschedules it to 9am;
  the completion-anchored mode moves the date, not the time.
- **All-day tasks are UTC midnight** and must stay there through a rollover.

---

## 2. Keyboard shortcuts

Canonical: `astrid-web/hooks/useKeyboardShortcuts.ts` (`KEYBOARD_SHORTCUTS`).
Locked by: `contracts/fixtures/shortcuts.json`, generated from that file — including the
`if (selectedTask)` guard, which is read from the dispatch switch rather than the table.

The scheme is bare-key and modifier-less, so muscle memory transfers between platforms. 27 keys
across 24 actions: Delete and Backspace share one, and `j`/down and `k`/up alias.

- **Ctrl-accelerators are additive and are not in this table.** `Ctrl+K` for the palette, `Ctrl+1..9`
  for list jumps, `Ctrl+N`, `Ctrl+Z`, `Ctrl+,` are Windows conventions layered on top. A bare key
  from the shared set must never be shadowed by one.
- **Arrow keys resolve under either name.** The table stores them as glyphs, the way web and Mac
  write them; the core also accepts `ArrowUp` and friends so a Windows `VirtualKey` needs no
  translation before asking.
- **The guard is half the contract.** Shortcuts do not fire while a text field or editor has focus,
  or while a modal is open. A key that fires under a dialog navigates the user away from what they
  were doing.

---

## 3. The session credential

Canonical: `astrid-ios/Astrid App/Core/Authentication/SessionCookie.swift`, ported here with its
tests as `astrid_core::auth::session_cookie`.

Secure storage holds a whole `Cookie` request header, not a bare token, and the server returns a
bare JWT when it renews a session. The renewed value is swapped **inside** the stored header:

- Keep whichever cookie name is already stored. Production uses the `__Secure-` prefix and
  development does not; the server accepts either.
- Keep the other cookies. The CSRF cookie travels here too, and dropping it breaks the next write
  rather than the next read — a far more confusing failure than an outright sign-out.
- Split on the first `=` only. Base64url padding puts `=` inside the value, and splitting on every
  one truncates the token silently.
- Never store a bare token. It would be sent as a nameless `Cookie` header, the server would find no
  session, and the user would be signed out on the very launch meant to keep them signed in.

---

## 4. Adding a contract

1. Change the canonical implementation in astrid-web, with tests.
2. Teach `contracts/export-from-web.mjs` to export the cases, and regenerate.
3. Port or update the Rust module until its tests pass against the new fixture.
4. Mirror into astrid-ios, which covers both iOS and Mac.
5. Add a row to the table in [ASTRID.md](./ASTRID.md) §2.

If the clients cannot be aligned in one change, write the divergence down here — with which
behaviour this crate follows and what it would take to close it. An undocumented divergence becomes
a bug report from a confused user.
