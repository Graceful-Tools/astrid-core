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

### D4 — web's custom-pattern path depends on the server's timezone

**This crate is UTC.** The fixture is generated with `TZ=UTC` so it records one defined behaviour.

Web's *simple* patterns use `setUTC*`, but its *custom* patterns use local date methods —
`setMonth`, `getDay`, `new Date(year, month, 1)`. Measured on a machine at UTC-8, "the third
Tuesday of the month" comes back as `2024-02-20T08:00:00Z`: local midnight, not UTC midnight. The
same input on a UTC machine gives `2024-02-20T00:00:00Z`.

So the result of a custom rollover depends on where the code runs. Two users completing the same
task from clients in different zones get instants eight hours apart, and for an all-day task the
displayed date can differ by a day. It also means the answer changes if the server moves.

Weekly patterns happen to be unaffected — they advance by whole days from an anchor, and the
weekday of a UTC instant is the same in any zone that does not shift it across midnight — but that
is luck, not design.

Closing it means moving web's custom path onto UTC methods, with the progression tests re-run under
a non-UTC `TZ`. Until then this crate matches the UTC answer, which is what web produces when
deployed in UTC.

### D5 — "this date does not exist next period" has two answers on web

**This crate matches web case by case**, which means it clamps in exactly one place and overflows
everywhere else. The fixture is what forced that: each of these was found by the generated cases
disagreeing with a port that clamped consistently.

| Step | Web | Example | Apple | Here |
|---|---|---|---|---|
| Simple monthly | **clamps**, with an explicit `setUTCDate(0)` | Jan 31 → **Feb 29** | clamps | clamps |
| Simple yearly | overflows (`setUTCFullYear`) | Feb 29 2024 → **Mar 1 2025** | clamps | overflows |
| Custom monthly, same date | overflows (`setMonth`) | Jan 31 → **Mar 2** | clamps | overflows |
| Custom monthly, same weekday | overflows, then searches that month | 5th Sunday from Dec 29 → **Feb 2** | clamps | overflows |

The custom monthly row is the one worth staring at. A task set to repeat on the **31st of every
month skips February entirely** and lands on March 2nd, because JavaScript rolls the overflow
forward rather than clamping. A user who set "the 31st" reasonably expects the end of February, and
that is what web's own *simple* monthly step would give them — the same product question, answered
two ways a few files apart.

Clamping is very likely right in all four rows: an anniversary on February 29th belongs on February
28th, and a monthly task on the 31st belongs on the last day of the month. But changing any of them
here alone would make the same task land on a different date depending on which app the user
completed it in, which is worse than the inconsistency. It changes on web first, then everywhere.

Evidence: the web half is **measured** — it is what `contracts/fixtures/repeating.json` records
from running web's own calculator. The Apple column is **read** from
`RepeatingTaskHandler.swift`, which uses `Calendar.date(byAdding:)`; Foundation clamps an invalid
result to the last valid day. Worth confirming on a device before the cross-repo fix, since the
point of that fix is to make three clients agree.

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
- **On first sign-in there is nothing stored to learn the name from**, and the two names are not
  interchangeable. The exchange response therefore states `sessionCookieName`, and
  `replacing_token_named` uses it. Assuming the production name against a dev server — or the
  reverse — signs in successfully and then reads as signed out on the very next request.

---

## 4. Desktop hand-off sign-in

Canonical: `astrid-web/lib/auth/desktop-handoff.ts` and the two routes it serves.
Here: `astrid_core::auth::desktop_handoff`.

The app cannot host the sign-in page, so it opens the system browser at `/auth/desktop`, the user
signs in with whatever the web already supports, and the browser returns a one-time code through
`astrid://auth/callback`.

**Any local program can register the same URL scheme.** That single fact produces every rule here,
and both halves enforce them independently:

| Rule | Server | Here |
|---|---|---|
| S256 only; `plain` is refused | `validateGrantRequest` | `CODE_CHALLENGE_METHOD`, no other path |
| The redirect URI is a per-client constant, never read from a request | `DesktopClient.redirectUri` | `CALLBACK_HOST` + `CALLBACK_PATH` |
| `state` is compared before the code is used | echoed, not trusted | `parse_callback` refuses on mismatch **and on absence** |
| A code is single-use and dies in five minutes | conditional-write claim | — |
| A wrong verifier burns the code | claim precedes verification | — |
| The verifier never leaves the client | — | only its SHA-256 is ever sent |

Two details worth keeping straight, because both are easy to get wrong in a way that only fails
against a live server:

- **The challenge hashes the verifier's ASCII bytes**, not the entropy the verifier was encoded
  from. RFC 7636 §4.2. The test locks the published Appendix B vector.
- **`astrid://auth/callback` parses as host `auth`, path `/callback`.** Both are checked, so
  `astrid://task/123` — a real activation this app receives — cannot be mistaken for a sign-in.

The state comparison is not constant-time and does not need to be: `state` binds a callback to the
flow that started it, and it travels in the same URL as the code anyway. The secret is the verifier.

---

## 5. List permissions

Canonical: `astrid-web/lib/list-permissions.ts`.
Apple: `astrid-ios/Astrid App/Models/TaskList.swift` (`role(for:)`) and `Core/Lists/ListPermissions.swift`.
Here: `astrid_core::permissions`, locked by `contracts/fixtures/permissions.json`.

**This crate follows web**, which the fixture makes literal: the expected answers are produced by
running web's own functions over the case matrix.

Precedence is the part worth stating, because it is invisible from any single predicate: ownership
beats an admin membership, an admin membership beats a plain one, and **any** membership beats the
public-viewer fallback. That last step is what stops a real collaborator on a public list being
silently downgraded to read-only.

Two answers surprise people, and both are deliberate on web:

- **A viewer may edit their own task on a public *collaborative* list**, and a **member may not edit
  someone else's**. Authorship, not role, decides on that one list type. Copy-only lists are the
  other way round: role decides and authorship is irrelevant.
- **Ownership is `ownerId` OR the `owner` relation.** Payloads exist that carry the relation and a
  different id; a client comparing only `ownerId` locks the real owner out of their own list.

### D6 — the Swift port resolves roles more strictly than web

**This crate follows web. The divergence is on the Apple side, and it costs real users access.**

`TaskList.role(for:)` matches membership rows with `$0.role == "admin"` and `$0.role == "member"`,
exactly and case-sensitively, and matches the member only on `userId`. Web lowercases the role,
treats presence in `listMembers` as membership whatever the role says, and also matches on the
nested `user.id`.

| Membership row | Web | Apple | Here |
|---|---|---|---|
| `role: "admin"` | admin | admin | admin |
| `role: "ADMIN"` | admin | **none**, or viewer on a public list | admin |
| `role: "MEMBER"` | member | **none**, or viewer on a public list | member |
| unrecognised or empty role | member | **none**, or viewer on a public list | member |
| stale `userId`, correct `user.id` | member | **none** | member |

The uppercase rows are not hypothetical: `app/api/v1/lists` created members as `'MEMBER'`, which is
why web was changed to lowercase in the first place (astrid-web task e2803305). Every user added
through that path is, on Apple, either locked out of a private list or silently demoted to a viewer
on a public one — able to see the list and unable to do anything with it, with no error explaining
why.

Evidence: the web column is **measured** — it is what `contracts/fixtures/permissions.json` records
from running `lib/list-permissions.ts`. The Apple column is **read** from `TaskList.swift`; it
should be confirmed against a device before the fix, since the point of the fix is to make three
clients agree. Worth a task in the iOS queue.

### The role a client cannot compute

Web derives a role from three more places, and **none of their fields exist on `V1List`**, the shape
a client receives from `/api/v1/lists`:

| Source | Fields it needs |
|---|---|
| Project owner or project member (cascades to every list in the project) | `project.ownerId`, `project.members` |
| Sibling membership on a **status** list (a board column) | `project.lists[].listMembers`, `listType` |
| Legacy denormalised `admins` / `members` arrays | `admins`, `members` |

So a list reached purely through project membership arrives with the user in none of its
`listMembers`, and every client computing a role locally sees **no access at all** — for a list the
server was happy to return. The visible effect is a list that renders as read-only, or whose
controls are all disabled, for someone who is a full collaborator on the board.

This is not something a client can fix. Either `/api/v1/lists` gains a resolved `role` field for the
requesting user — the cleaner answer, since the server has already done the work to decide the list
is visible — or the project relations join the payload. Until then this crate answers only what the
wire shape can support, which is why the fixture does not contain those cases: locking in answers no
client can produce would be worse than the gap.

---

## 6. List filtering and sorting

Canonical: `astrid-web`'s list view; shared on Apple by
`astrid-ios/Astrid App/Core/Filters/ListTaskFiltering.swift`, which iOS and Mac both call. Here:
[`astrid_core::filters`]. Not yet fixture-locked — the exporter cannot run web's list view — so it
is a port with tests rather than a generated contract.

The governing rule on all three clients is that **an unrecognised filter value keeps everything**.
These values are stored on the server and synced between clients, so a build from six months ago
will meet values it has never heard of, and treating one as "matches nothing" empties somebody's
list on their screen for no visible reason.

### D7 — an unrecognised due-date filter hides undated tasks

Every client answers a task with **no due date** before it looks at the filter value:

```
guard let dueDateTime = task.dueDateTime else { return filter == "no_date" }
```

So for a value none of them recognise, dated tasks are kept (the `default:` arm returns true) and
undated ones are dropped. The rule holds for every other filter and fails for this one.

- **Where it bites:** a client older than a due-date filter value the server has learned shows a
  list with every undated task missing. Undated tasks are the majority in most lists.
- **This crate follows the existing behaviour**, reproduced deliberately with a test that says so
  — a client that fixed it alone would show a different list from the other two, which is worse
  than the bug.
- **The fix is one line on each of three clients**: answer the undated case inside the `match`, so
  an unknown filter falls through to "keep it" like everything else. Web first, then here, then
  astrid-ios.

### D8 — the two Apple clients order the assignee picker differently

iOS (`AssigneeOptions.build`) sorts **agents first, then you, then everyone by name**, and offers no
unassigned row — its picker adds one in the view. The Mac (`MacAssigneeOptions.build`) offers
**"no one" first, then you, then everyone by name**, and has no agents at all: its picker predates
agents being assignable, so an account's agents cannot be chosen from the Mac detail pane.

- **Where it bites:** the same task, opened on an iPhone and on a Mac, offers a different set of
  people in a different order. On the Mac an AI agent cannot be assigned from the detail pane at
  all, though a task already held by one displays correctly.
- **This crate takes iOS's ordering and the Mac's unassigned row.** iOS's is the one written
  deliberately to stop surfaces drifting (task 1484ea4a), and unassigned is a real choice — a
  picker that cannot express it cannot take a task off somebody. One function,
  `astrid_core::rows::assignee::options`, answers for every surface.
- **The fix is on the Mac**: build its options from the same rule, which is one call once the agent
  roster is available to it.

---

## 7. Adding a contract

1. Change the canonical implementation in astrid-web, with tests.
2. Teach `contracts/export-from-web.mjs` to export the cases, and regenerate.
3. Port or update the Rust module until its tests pass against the new fixture.
4. Mirror into astrid-ios, which covers both iOS and Mac.
5. Add a row to the table in [ASTRID.md](./ASTRID.md) §2.

If the clients cannot be aligned in one change, write the divergence down here — with which
behaviour this crate follows and what it would take to close it. An undocumented divergence becomes
a bug report from a confused user.
