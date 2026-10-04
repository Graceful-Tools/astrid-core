# Cross-platform contracts, and where the clients disagree

Rules that must behave identically on web, iOS/Mac and Windows. The mechanism is in
[`contracts/README.md`](../contracts/README.md); this file records the **decisions and the known
divergences** — the things a generated fixture cannot tell you.

A divergence listed here is not a Windows bug to fix locally. Fixing one means changing every
client together, and the entry says which behaviour this crate currently follows and why.

> **On disagreement, follow iOS** (Jon, 2026-10-03). Where this crate's behaviour differs from the
> iOS app's, the crate changes to iOS's behaviour, and the web adopts it as it moves onto the core.
> This overrides "web is canonical" for *disputed* behaviour only: a rule the clients agree on
> still changes on web first, and the fixtures in `contracts/fixtures/` are still generated from
> web and never hand-edited. Entries settled this way say "Resolved 2026-10-03 toward iOS".

---

## 1. Repeating-task rollover

Canonical: `astrid-web/types/repeating.ts` (`calculateTaskNextOccurrence`) and
`astrid-web/lib/repeating-task-handler.ts`.
Apple: asks this crate (`rules` → `completion` / `nextOccurrence`) since 2026-09-28.
Web: its server asks this crate (`rules` → `nextOccurrence`, WebAssembly) since AWTD-1063, with
the TypeScript as fail-safe; the browser computes no rollover.
Here: `astrid_core::repeating`.

**Under the follow-iOS rule (2026-10-03), D1–D5 needed no change here:** the Apple apps already
ask this crate for repeating math, so iOS's answer *is* this crate's answer. Web adopted it in
AWTD-1063 (D1's timed-task zone, D3's whose-midnight, D4's server-zone dependence): its TypeScript
now steps in a named zone exactly as this crate does, the browser sends its zone with a
completion, and `repeating.json` locks the zoned answers (`completions`, `zonedProgressions`) from
web's own calculator. Where this crate reproduces web deliberately (D2, D5), iOS reproduces it
too, so there is no iOS-vs-core disagreement to resolve.

### D1 — Whose calendar a step is taken on

**Settled 2026-09-28: all-day tasks on UTC's, timed tasks on the person's. Web adopted it
2026-10-03 (AWTD-1063)**: the browser sends `timeZone` with a completion and the server steps a
timed task on that calendar; without a zone (API clients, MCP), UTC's, the old answer. Every calendar step in
`astrid_core::repeating` takes a zone (`chrono_tz::Tz`) and is done on that zone's wall clock.

- **All-day tasks step in UTC.** An all-day date is stored as UTC midnight and names a day, so
  UTC is its calendar — exactly web's `setUTC*` arithmetic, and what every contract fixture checks.
- **Timed tasks step in the person's zone.** "Monthly at 2pm" is 2pm in March as well as in
  February, and "until the 15th" ends on their 15th. Stepping a timed task in UTC — what web does,
  because its server runs in UTC — moves it an hour at every daylight-saving change.

The Apple apps always stepped months and years in the device's calendar (`Calendar.current`);
they now ask this crate, which answers the same for timed tasks and correctly for all-day ones,
which the old Swift did not. Web follows since AWTD-1063; Windows by nothing more than taking
this crate.

### D2 — Weekly patterns ignore their interval

**Reproduced deliberately.** `astrid_core::repeating::next_weekday_occurrence` ignores `interval`.

Both web and Swift ignore `interval` for weekly custom patterns: `getNextWeekdayOccurrence` takes the
interval and never reads it. So "every 2 weeks on Mon and Wed" advances every week on all three
clients. The stored pattern says one thing and every client does another.

Diverging here would be worse than the bug: this client would schedule occurrences the others do not
have, on a field the user believes is shared. It changes on web first.

### D3 — "Same weekday of the month" drops the time of day

**Reproduced deliberately — on the person's calendar** (web too, since AWTD-1063).

Both web and Swift build this date from the first of the target month, which is midnight, and then
add days — so a task due at 10am on the third Tuesday rolls over to midnight on the next third
Tuesday. Same reasoning as D2: dropping the time is web's to fix, and until then matching matters
more.

Whose midnight, though, is D1's question. Web's is UTC midnight, which in California is Monday
evening — a "third Tuesday" task shown on a Monday. Swift's was local midnight, the Tuesday the
person chose. This crate takes the midnight of the zone it steps in (D1): UTC for an all-day task,
the person's zone for a timed one.

`same_date` monthly patterns are unaffected — they keep their time.

### D4 — web's custom-pattern path depends on the server's timezone

**Closed 2026-10-03 (AWTD-1063).** Web's calculator no longer reads the machine's zone: every
step is taken in the zone the request names, as here, and the progression cases pass under any
`TZ` (web runs its tests under UTC, Tokyo, Los Angeles and Lord Howe). What follows is the history.

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
from running web's own calculator. The Apple column records the
pre-2026-09-28 Swift calculator (`RepeatingTaskHandler.swift`, now a facade over this crate), which used `Calendar.date(byAdding:)`; Foundation clamps an invalid
result to the last valid day. Worth confirming on a device before the cross-repo fix, since the
point of that fix is to make three clients agree.

### Stored patterns web read differently (closed toward this crate, AWTD-1063)

Moving web onto this crate's arithmetic surfaced five readings of a malformed or partial stored
pattern where web's old TypeScript and this crate disagreed. None is reachable from web's editor;
an API or MCP write can store them. Web now answers as this crate does (it is what iOS runs), and
`repeating.json` locks each:

| Stored | Old web | Here (and web now) |
|---|---|---|
| `endAfterOccurrences: 0` | ignored (falsy): repeats forever | ends at the next completion |
| custom yearly `month`/`day` from an anchor on a day the target month lacks (e.g. 31 Jan → "10 Feb") | `setMonth` overflowed first, then `setDate`: **10 March** | 10 February |
| custom monthly `same_date` with no `monthDay` | invalid: series ended | steps a month (the day was never read) |
| custom pattern with no `interval` | Invalid Date written | series ends |
| custom `interval` < 1 | invalid: series ended | **days**: steps that many days (0 = same date, the series never advances); **months** < 0: ends |

One reading every client shares is a live bug, not a divergence: a custom `months` pattern with
no `monthRepeatType` (`{ unit: "months", interval: 6 }`, ten such tasks in production on
2026-10-03, none written by web's editor) **ends the series at its next completion** on web, iOS
and here alike. Reading it as `same_date` is the likely fix; it is a rule change for every client,
so it is recorded (`repeating.json` locks today's answer) rather than made here.

The last row of the table is the one answer here worth changing: an interval of 0 days re-opens the task on
the same date forever. Which way to fix it (end the series, or read it as 1) is a product decision
for every client at once; until then web matches.

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

**Resolved 2026-09-28: the Apple apps ask this crate** (`rules` → `listAccess`), so all three
clients answer as web does. Kept for the history below.

**This crate follows web. The divergence was on the Apple side, and it cost real users access.**

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
| Project owner (as **admin**, never owner) or project member (cascades to every list in the project) | `project.ownerId`, `project.members` |
| Sibling membership on a **status** list (a board column) | `project.lists[].listMembers`, `listType` |
| Legacy denormalised `admins` / `members` arrays | `admins`, `members` |

**This crate now resolves all three when it is given them** (AWTD-1061, 2026-10-03): `ListAccess`
takes optional `listType`, `project { ownerId, members[], lists[].listMembers[] }`, `admins` and
`members`, and `contracts/fixtures/permissions.json` locks every branch and its precedence as web
runs it — list ownership, then an admin membership, then `admins`, then any membership, then
`members`, then the project, then the public viewer fallback. astrid-web's server is the caller
that sends these fields: its list permissions decide through this crate with the project loaded.

**Where a list role and a project role meet, the higher one wins** (Jon, 2026-10-04). A plain
list member — a `listMembers` row of any non-admin role, or the legacy `members` array — who owns
or administers the project is an **admin** of the list. The project's ceiling is admin, never
owner, so that person still cannot delete a list somebody else owns. A plain project member gains
nothing over a plain list membership, and a list owner or list admin is never lowered.

| On the list | On the project | Role on the list |
|---|---|---|
| owner | anything | owner |
| admin (row or `admins`) | anything | admin |
| plain member (row or `members`) | owner | **admin** (was member until 2026-10-04) |
| plain member (row or `members`) | admin | **admin** (was member until 2026-10-04) |
| plain member (row or `members`) | member, or status-list cascade | member |
| none | owner or admin | admin |
| none | member, or status-list cascade | member |

Until that decision the list membership was consulted first and its answer returned, so such a
person was only a **member** of the list; web and this crate changed together and the fixture
records the new answer.

**A client still cannot.** The fields are not on the wire, so a list reached purely through
project membership arrives with the user in none of its `listMembers`, and a client computing a
role locally sees **no access at all** — for a list the server was happy to return. The visible
effect is a list that renders as read-only, or whose controls are all disabled, for someone who is
a full collaborator on the board. A client that sends none of the new fields gets exactly the
answers it got before.

Closing that is a wire change, not a rules change: either `/api/v1/lists` gains a resolved `role`
for the requesting user — the cleaner answer, since the server has already decided the list is
visible — or the project relations join the payload, which this crate can now read as-is.

---

## 6. List filtering and sorting

Canonical: `astrid-web`'s list view; shared on Apple by
`astrid-ios/Astrid App/Core/Filters/ListTaskFiltering.swift`, which iOS and Mac both call. Here:
[`astrid_core::filters`]. **Fixture-locked since 2026-10-03** by `contracts/fixtures/filters.json`
(the due-date filter, the completion filter with every window kind, and the sort) and
`manual-order.json`: the driver runs the pure functions web's list view calls
(`hooks/useFilterState.ts` → `applyDateFilter`, `shouldShowCompletedByFilter`, `sortTasksForList`),
since the hook itself cannot be imported. Where iOS and web disagree the core follows iOS and the
cases sit in the fixture's `disputed` array under D7 and D33–D36.

The governing rule on all three clients is that **an unrecognised filter value keeps everything**.
These values are stored on the server and synced between clients, so a build from six months ago
will meet values it has never heard of, and treating one as "matches nothing" empties somebody's
list on their screen for no visible reason.

### D7 — an unrecognised due-date filter hides undated tasks

iOS answers a task with **no due date** before it looks at the filter value:

```
guard let dueDateTime = task.dueDateTime else { return filter == "no_date" }
```

So for a value it does not recognise, dated tasks are kept (the `default:` arm returns true) and
undated ones are dropped. The rule holds for every other filter and fails for this one.

**Measured 2026-10-03, web does not do this**: `applyDateFilter`'s `default` keeps every task,
dated or not (`filters.json`, disputed under D7). Under the follow-iOS rule this crate keeps iOS's
answer, so the paragraph below now describes an iOS-and-core behaviour that web does not share.

- **Where it bites:** a client older than a due-date filter value the server has learned shows a
  list with every undated task missing. Undated tasks are the majority in most lists.
- **This crate follows iOS**, reproduced deliberately with a test that says so.
- **The fix is one line on iOS and here**: answer the undated case inside the `match`, so an
  unknown filter falls through to "keep it" like everything else — which is what web already does.

### D8 — the two Apple clients order the assignee picker differently

**Resolved on the Mac (AITD-401):** `MacAssigneeOptions.build` now calls the shared
`AssigneeOptions.build` with the agent roster and adds its unassigned row, so both Apple
clients answer as this crate does. Kept for the history.

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

### D9 — auto-link duplicates a list on Apple when the list cannot reach the server

Apple's auto-link creates the Astrid counterpart of a remote list by awaiting
`ListService.createList`. Offline that returns a temporary id, Apple skips the link with a message,
and — crucially — filters temporary ids out of the lists that may be *adopted* on the next pass. So
the next pass sees no list of that name, creates another, and does so again every five minutes
until the connection comes back.

- **Where it bites:** somebody who turns on an all-lists mode on a train ends up with several lists
  called "Groceries", one per pass, and has to delete them by hand.
- **This crate lets a pending list be adopted.** A list still carrying a temporary id stays in the
  adoptable set, so the next pass adopts the one already made rather than making another; it is
  still refused as a *link* target, for the reason Apple refuses it — a link attached to an id
  about to change is a link to nothing. `AutoLinkReport::waiting_to_be_created` counts them.
- **Why diverge here at all**, when the rule is to port faithfully: the module's own documentation
  says duplicating somebody's list "is not a small bug — it duplicates somebody's list on every
  pass until they notice", and reproducing it would mean shipping that on purpose. The planner's
  own adoption rules already express the fix; only Apple's temp-id filter stands in the way.
- **The fix on Apple** is the same one-line change: keep temporary ids in the adoptable set and
  refuse them only at the link step.

---

## 7. Adding a contract

1. Change the canonical implementation in astrid-web, with tests.
2. Teach `contracts/export-from-web.mjs` to export the cases, and regenerate.
3. Port or update the Rust module until its tests pass against the new fixture.
4. Bump astrid-ios's pinned revision, which covers both iOS and Mac; mirror only what Swift still owns.
5. Add a row to the contracts table in astrid-ios `ASTRID.md` §8.

If the clients cannot be aligned in one change, write the divergence down here — with which
behaviour this crate follows and what it would take to close it. An undocumented divergence becomes
a bug report from a confused user.

---

## D10 — A list's *When* default without a *When Time* stamps the creation instant on web

**This crate stores the calendar day.** `astrid_core::services::list_defaults`.

A list can default its new tasks to *today*, *tomorrow* or *next week* (`defaultDueDate`) and to
a time of day (`defaultDueTime`, or `null` for all day). When the date default is set and the
time default is absent, web's `applyListDefaults` takes `parseRelativeDate("today")`, which is
`new Date()` — the moment the task was created, 14:37 and all — and leaves `isAllDay` false. A
"due today" default produces a task due at whatever time it was added.

Here that task is **all-day on the reader's calendar day**, stored the way all-day dates are
stored (`date::all_day_instant`), which is what the setting says and what the task that filed
this asked for. The web should do the same; until it does, the two clients disagree only in
this one corner, and only about the time of day of a task the person never gave a time.

## D11 — Smart parsing: closed 2026-09-11

**This crate now reads what web reads.** `astrid_core::parse::smart`, applied in the `createTask`
command when the title came from the quick-add box and the account's `smartTaskCreationEnabled`
is on, ports `parseTaskInput` (`lib/task-manager-utils.ts`) in the web's order: `#list` tags,
"weekly mon and wed", "daily" / "every month", "tomorrow" / "next week" / a weekday, and
"urgent" / "high priority" — in all twelve languages, whose keyword tables are carried in
`contracts/fixtures/smart.json` rather than retyped. The same fixture records the web's answers
for 220 inputs under a pinned clock, and `tests/smart_contract.rs` replays them.

Two things were found on the way and are reproduced faithfully rather than fixed here: "low
priority" strips the words and sets **no** priority, because the web reads `priority || undefined`
and zero is falsy; and "Daily Mail subscription" becomes "Mail subscription", repeating daily,
because a keyword is matched wherever it sits on a word boundary. Both change on web first.

### D12 — a date word is a calendar day here and an instant on web

**Resolved on Apple 2026-09-28:** the Apple apps parse through this crate (`rules` →
`smartParse`) and store the day at UTC midnight. Their English-only Swift parser stored local
midnight, which east of UTC is the previous UTC day — "tomorrow" typed in Paris landed on today.

The web's `parseRelativeDate("tomorrow")` is `new Date()` plus a day — 14:37 tomorrow, if that is
when the task was typed — and the task is not all-day. Here the same word gives **tomorrow as an
all-day task**, stored the way every all-day date is (`date::all_day_instant`), for the reason
D10 gives about list defaults: the person said a day, not a time. The fixture compares the
calendar day, which the two agree on.

### D13 — "assign to" and "for" are not read here

The web's last step strips `(assign to|assigned to|for) <word>` from the title and assigns the
task to the signed-in user if the word contains "jon" — a stub, as its comment says. It also
turns "Buy flowers for mum" into "Buy flowers". This crate leaves the phrase in the title and
assigns nobody. Porting a stub that eats "for mum" would be worse than the gap; the driver leaves
such inputs out of the fixture so the divergence is this paragraph and not a failing test.

### D14 — a list without a picture draws nothing here, a default icon on web

The web resolves a list's picture as `imageUrl || coverImageUrl || a default icon hashed from
the list id` (`lib/default-images.ts`), and draws the result over the list and as the sidebar's
mark. This client draws the picture beside the list's name only when the list has one, and its
sidebar keeps the colour discs. The stored value is the same on both — a chosen picture set here
shows on the web and the other way round (task 3a913e52) — so the divergence is what an absent
picture looks like, not what a present one is.

### D15 — the copy control is for a public list the reader cannot edit, not every public list

The web draws its copy control in place of the checkbox on any task in a PUBLIC list
(`lib/public-list-utils.ts` `shouldShowCopyButton`), owners included, and copies with no target,
so the copy lands with the person. This client draws it only when the ported permission rule
(`permissions::can_edit_task`) says the reader may not edit that task: a passer-by on a public
list, or, on a collaborative one, a task somebody else wrote. The owner, the admins and the
members keep their checkboxes, since they can complete. The copy itself is the same request
with no target (task f6bc59e8).

### D16 — full screen is never offered on a board card here; the web now offers it

`PRODUCT_CONTRACT.md` §3 says full-screen task detail is **never offered on the inline/board
panel**, which is deliberately a peek. The web's own code has since opted board cards in (its
task 52bf1efb: "a board card's details are the only way to read a task on the board, so they
need the escape hatch most"), leaving the contract and the web apart. This client follows the
contract, as task 1927c2e7 asked: the expand control is drawn on the side pane only, and a card
that takes the detail ends full screen. If the contract is amended to match the web, the change
here is `ShellViewModel.CanEnterFullScreen` and the pane's place inside the card's slot.

### D17 — after a drag, tasks not on screen keep their place here; iOS puts them newest first

Every client sends the server a whole order and the server reconciles it (`lib/list-manual-order.ts`,
`sanitizeManualOrder`): unknown ids dropped, repeats collapsed, anything unnamed appended in
creation order. What differs is how each client fills in the tasks a drag did not touch — the
ones hidden by a filter or outside the page. iOS (`TaskListView.moveTask`) appends them newest
first. The web moves one task relative to another inside the existing arrangement. This client
(`astrid_core::manual_order::arranged`) leads with the rows as they came to rest, then the rest
of the existing arrangement in its old order, then anything never arranged by creation — the
same answer the server would produce from the same request, so the screen does not jump when
its answer lands. The stored result is the server's either way; only the offline redraw differs.

### D18 — the blocker picker searches this machine's cache; web searches the server

`TASK_BLOCKING_DEPENDENCIES.md` says the "Wait on a task…" picker uses
`GET /api/v1/search` and "does not filter client-side over loaded tasks", because a web picker
that filtered the page it had already loaded offered only what happened to be on screen (web task
`5df85b9f`). `Command::TaskBlockerCandidates` searches the cache instead, through
`services::search::search` — the same path `Command::SearchTasks` takes, because this core has no
server-search endpoint at all.

Two reasons this is a divergence rather than the same bug. It is not one: the cache is the whole
synced account, not a loaded page, and it uses the shared search grammar, so the result set is the
same one the server would rank differently. And it is what every search surface in this app already
does, so a picker that reached past it would be the only screen here whose results a train journey
changes.

What is genuinely lost is the server's permission filter *in the query*
(`listVisibilityWhere`). A task that synced to this machine before a share was revoked can still be
offered as a blocker; the write is then refused server-side, which is the right outcome reached
late rather than early. Growing a server-search path in the core would close it, and is its own
task rather than a detail of the Waiting on row (task 69a840a4).

### D19 — a dependency cycle is reported by the journal here, not by the picker

`TASK_BLOCKING_DEPENDENCIES.md` gives the picker a `cycleError` — "those tasks would end up
waiting for each other" — because web POSTs the link and reads the `409 dependency_cycle` in the
same click. `Command::AddTaskBlocker` cannot: the write is optimistic and journalled
(`services::dependency::add`), so it has already answered by the time the request goes, and the
`409` arrives in `outbox::handlers::add_task_blocker`, which dead-letters the entry.

So a client here has nothing to draw that copy from at pick time, and the Windows shell words its
one failure case — a refusal the core itself returns — as "could not wait on that task" instead
(`detail.waiting_on_error`).

This is very nearly unreachable rather than merely unreported: `pickable` already drops every id in
`dependentIds`, so the picker does not offer a task that would cycle. What remains is the race —
two devices adding opposite edges before either syncs — and a dead-lettered entry with the reason
on it, which is what every other refused write here also leaves behind. Saying it at the moment of
the pick would mean giving up the optimistic write, and blocking a task on a plane is worth more
than a message about a case the picker will not offer (task 69a840a4).

### D20 — a board column's add field is offered in Done here and on the Mac, never on web

`MacBoardView.addCardField(col)` puts an add field at the foot of **every** column, Done included,
and Windows copied that placement when it grew one (task 95c7a68f). Web does not:
`project-status-board.tsx` renders `isDoneColumn ? null : <form>`, so typing a new card straight
into Done is possible on two clients and not on the third.

The behaviour is deliberate on both sides rather than an oversight on one. A card typed into Done is
a thing somebody did and is recording afterwards — the commonest way a list gets an entry it never
had — and `dispatch::board::add_board_card` creates it and then completes it through
`TaskService::complete`, so a repeating card typed there rolls forward exactly as one dragged there
does. Web's reading is that a column meaning "finished" is a strange place to start something, and
its quick-add sits above the board instead.

Nothing in `ColumnCreate` decides this: the rule answers what a card in a column is made of, and
whether a column offers a field at all is each client's own layout. So this is a divergence to know
about rather than one to close — if it does close, it closes on web, by adding the field, not here
by taking it away.

### D21 — completing a repeating task counts the occurrence only on the web

**Server half closed (astrid-web AWTD-1035):** PUT `/api/v1/tasks/:id` accepts `occurrenceCount`
(a non-negative integer) when the same request does not make the server roll, and this crate's
`TaskService::complete` already sends it with the device-rolled due date. A client that rolls on
the device and does not send the count (the Apple apps' own write path, until they drive this
crate's services) still never ends an "after N times" series. What follows is the original entry.

The web completes by sending `completed: true`; the server then rolls the task forward itself
(`lib/repeating-task-handler.ts`) and increments `occurrenceCount`. Every other client — iOS, the
Mac, and this crate's `TaskService::complete` — rolls the task forward on the device and sends
the new due date with `completed: false`, so the server never runs its rollover, and the v1
update route does not read an `occurrenceCount` from the body. The count therefore never moves
for a completion made anywhere but the web.

- **Where it bites:** "end after N times" never ends for somebody who completes from a phone, a
  Mac or Windows; the series repeats forever.
- **This crate counts locally** (`repeating::completion` answers `RollForward` with the new
  count), so the cache is right until the next pull overwrites it with the server's.
- **The fix is on the server first:** accept `occurrenceCount` on the v1 update, or let clients
  send the completion itself (`completed: true` plus `localCompletionDate`) with an idempotency
  key so a retried delivery cannot roll a task twice. Then every client sends it.

### D22 — the Apple completion filter reads values this crate does not

**Resolved 2026-10-03 toward iOS** (Jon: on disagreement follow iOS). `filters::recently_completed::should_show_completed` / `should_show_open`
now read iOS's values (`RecentlyCompletedPresets.applyCompletionFilterWithWindow`, which is also
web's `shouldShowCompletedByFilter`): `completed` (only finished tasks), `incomplete` (only open
ones), `all`, and `default` (open plus recent completions in the window) — with `hide`, `show`,
absent and any unknown value read as `default`. The picker (`rows::filter_picks`, `COMPLETION`)
offers iOS's four in iOS's order: default / all / completed / incomplete (new keys
`filter.completion.completed` and `filter.completion.incomplete`; `filter.completion.hide` is
gone). Tests: `d22_completion_filter_reads_ios_values`, `d22_completion_picker_offers_ios_choices`,
`an_unrecognised_filter_mode_reads_as_default`.

Previously: Apple knew `completed` / `incomplete` and read `hide` or unknown as `default`; this
crate knew only `hide` and `default` and kept everything for any other value, so a list saved on a
Mac as "completed" showed everything on Windows. Reading an unknown value as `default` still never
empties a list: open tasks stay.

### D23 — a mention on the Mac is plain text

`MacAutocomplete.insert` writes `@label ` rather than the `@[Name](id)` reference iOS and this
crate (`parse::mentions::insert`) write, so a mention typed on the Mac is never a reference: no
pill, no notification.

### D24 — snooze moves a different field, by different amounts

**Resolved 2026-10-03 toward iOS** (Jon: on disagreement follow iOS). `SnoozeReminder` (`app::dispatch::reminders::snooze_reminder`) now does
what `ReminderPresenter.snoozeTask` does: it writes `dueDateTime` = now + the snooze, as a timed
task (`isAllDay: false`), and **leaves `reminderTime` untouched** — iOS never writes it when
snoozing. The choices are iOS's: 15 minutes, a day, a week (`reminders::SNOOZE_CHOICES`, from
`ReminderView`), and the notification action's hour is `reminders::NOTIFICATION_SNOOZE_MINUTES`.
iOS reschedules its local notification for the snoozed time; the core's equivalent is a
device-local snooze mark (cache metadata `reminder.snoozed.<id>`), so the in-app reminder comes
back at the later of `reminderTime` and the mark (`reminders::reminder_at`). Tests:
`d24_snooze_moves_the_due_date_as_ios_does`, `d24_a_snoozed_reminder_comes_back_at_the_snoozed_time`,
`d24_snooze_choices_are_ios`, `a_snooze_mark_moves_the_reminder_on_this_device`.

Previously this crate moved `reminderTime` by 10/30/60/1440 minutes — which the v1 task PUT
ignores, so that snooze never reached the server; the due date does.

### D25 — My Tasks on iOS is only what is assigned to you

**Resolved 2026-10-03 toward iOS** (Jon: on disagreement follow iOS). `filters::my_tasks::filter` keeps only tasks assigned to the reader (iOS's
`task.assigneeId == currentUserId`; the nested `assignee` stands in for a missing id). Unassigned
tasks — even ones the reader created — are out, and signed out it is empty. `filterAssignee` is
carried in the preferences for the round trip but never applied, because iOS's My Tasks does not
read it. Tests: `d25_my_tasks_is_assigned_to_the_reader_only`, `d25_my_tasks_ignores_filter_assignee`.
The Mac (`MacMyTasks.filter`) still includes unassigned tasks it created; it should take this.

### D26 — Apple's due-date quick picks cannot clear a date, and pick local midnight

**Resolved 2026-10-03 toward iOS** (Jon: on disagreement follow iOS). `rows::due_picks::DATE_OPTIONS` is iOS's four — Today, Tomorrow, In 3
days, Next week — with no "No due date" row (`DueDateQuickPicks.dateOptions`), and
`dueDateOptions` no longer returns a clearing row; clearing remains `dueDateTime: null` on an
edit. Test: `d26_due_date_quick_picks_are_ios_four` (and the `dueDateOptions` dispatch tests).

**Storage is unchanged, because iOS agrees with it.** iOS's all-day quick pick
(`InlineDatePicker.setQuickDate`) takes the reader's local calendar day and writes it at UTC
midnight, and `TaskService.updateTask` normalises any all-day date to UTC start of day — the same
value `due_picks::all_day_pick` stores. Nothing needed flagging; the "local midnight" in this
entry's title described the picker's arithmetic, not the stored value.

### D27 — priority labels are one step apart

**Resolved 2026-10-03 toward iOS** (Jon: on disagreement follow iOS). The priority filter picks (`rows::filter_picks`, `PRIORITY`) now label
3/2/1/0 as Highest / High / Medium / Low, under the keys iOS's filter sheet resolves:
`lists.highest_priority`, `lists.high_priority`, `lists.medium_priority`, `lists.low_priority`
(iOS `Localizable.strings`: "!!! Highest", "!! High", "! Medium", "○ Low"). The stored numbers
are unchanged. Test: `d27_priority_labels_are_ios_words`. A shell that resolved the old
`priority.*` keys for these picks needs the four new ones. (The search grammar's
`priority:high` = 3 is web's fixture-locked vocabulary and is untouched.)

### D28 — each Apple list picker applies half of "can a task be filed here"

**Resolved 2026-10-03 toward iOS** (Jon: on disagreement follow iOS). `rows::list_picks::is_destination` now refuses only virtual lists, as
iOS's `InlineListsPicker` does; board-status lists and labels are offered. `ListService::destinations`
asks the same function, so there is one rule. Tests: `d28_list_picker_excludes_virtual_lists_only`,
`virtual_lists_are_not_offered_as_destinations`. The Mac's `MacListPicker` (excludes only status
lists) should take this.

### Subtask splice depth and list search — checked against iOS 2026-10-03

- **Splice depth cap: no divergence.** iOS's private splice in `TaskListView.filteredTasks` and the
  shared `SubtaskSplicing.swift` both stop following children at depth 10 (`depth < 10`), which is
  `filters::subtasks::MAX_SPLICE_DEPTH`. Pinned by `subtask_splice_depth_cap_matches_ios`.
- **Search: resolved toward iOS.** iOS's list search (`TaskListView.applySearchFilter`) matches the
  title, the description and the assignee's name. `services::search` (`search` and `matches`) now
  matches the assignee's name too — the task's `assignee`, or the cached person its `assigneeId`
  names — but not iOS's "Unknown User" placeholder. The structured grammar (`parse::search`, locked
  by the web-generated `search.json`) is untouched and still passes. Test:
  `search_matches_the_assignee_name_as_ios_does`.
- **Search box: now iOS's `TaskSearch.results` exactly (AITD-459, resolved toward iOS
  2026-10-03).** `Command::SearchTasks` answers through `services::search::search_tasks`, which
  differed from iOS four ways and now does not: it searched from **two** characters (iOS: one —
  only an empty query is nothing); it **included completed** tasks (iOS: a list's default
  completion filter — open, and completed inside the 24-hour window; an absent `includeCompleted`
  now means that, `true` everything, `false` open only); it **included subtasks** (iOS: top-level
  only); it sorted **title match, then open, then recency** (iOS: the `priority` list sort). A
  plain query is also iOS's phrase **as typed** — not trimmed, split or unquoted; only a query that
  uses the grammar is read through it. Kept as a superset of iOS: the web's grammar (`is:open`,
  `assignee:me`, …) and a bare identifier as a direct hit. One deliberate difference stays: the
  assignee name never matches iOS's "Unknown User" placeholder. The blocker picker's cache search
  (`services::search::search`, D18) is unchanged — two characters, completed included. The Apple
  apps search through this command and their Swift `TaskSearch` is deleted. Tests: `aitd459_*` in
  `services::search` and `app::dispatch::tests`.

### D29 — Apple refused members of a public copy-only list

Web (`lib/list-permissions.ts`, and so the permissions fixture) lets a **member** of a public
copy-only list add tasks and edit any task in it; only passers-by (viewers) are sent to copy the
list. Apple's `ListPermissions` refused members as well, so a member of such a list could neither
add nor edit on a phone or a Mac what they could on the web. **Resolved 2026-09-28**: the Apple
apps ask this crate, which answers as web does.

### D30 — "low priority" set priority 0 on Apple; the web sets none

Web's `parseTaskInput` answers `priority || undefined`, so "low priority" (0) is no priority: the
words leave the title and the picker or the list's default decides. Apple's parser answered 0,
which overrode a list whose default is high. **Resolved 2026-09-28:** Apple asks this crate.


### D31 — membership changes offline: Apple queued them, the core refused them

The Apple apps queued an invitation, a role change or a removal made offline and sent it when
the network returned; the core (and so Windows) refused all three offline, on the grounds that
a queued invitation would show a member who does not exist to every permission check. Web is
online-only by nature. **Resolved 2026-09-28** in favour of the queue, without the hazard:
every change is sent at once and a refusal fails the command, as before; only when the request
never reached the server is it journalled (`services::members`). A queued invitation is kept
among the list's pending **invitations**, which no permission check reads, never among its
members; the server's answer replaces it. Changes to one list keep their order in the list's
lane.

### D32 — waiting on a task: Apple was online-only, the core journalled

Apple's `TaskBlockerService` sends an add or a removal at once and shows a refusal — a cycle is
refused by the server — as `tasks.waitingOn.addError`, and asks the server's
`GET /api/v1/search` for picker candidates. The core journals both writes optimistically, so a
refused cycle becomes a dead letter the person never sees, and searches its own cache for
candidates. **Resolved 2026-09-28** as D31 was: an add or a removal is sent at once and a
refusal fails the command; only a failed network journals it. The picker asks the server's search
first and falls back to this cache offline.

## Phase 0: what the new fixtures found (2026-10-03)

Four rules the core had ported with no fixture behind them — markdown, filters and date windows,
manual order, reminders — are now locked by web-generated fixtures (`contracts/README.md`). Each
driver routes the cases where the core deliberately follows iOS to its fixture's `disputed` array,
naming the entry below; the tests require each entry still to disagree with web somewhere, so an
entry cannot outlive the divergence.

Two drifts were plain bugs in the core and were fixed rather than recorded: **"Recently
completed" (`completedAt`) sorted as auto here**, while iOS and web both lead with what was
finished last (task c6e87fb8; `filters::sort_by_setting`) — the sort picker
(`rows::filter_picks`) still does not offer it, which a shell adding it needs a string for; and
four markdown corners listed under D38.

### D33 — sort orders: the core's are iOS's

**Resolved 2026-10-03 toward iOS** (Jon: on disagreement follow iOS); the core already answered
this way. iOS (`sortTasksByListSetting`) and this crate differ from web's `sortTasksForList` in:

| Order | Web | iOS / here |
|---|---|---|
| `priority` | priority only — a finished task can sit between open ones | completed sink, then priority, then due date |
| `when` | reads the legacy `when` / `dueDate` fields, which v1 tasks do not carry, so it keeps the input order | due date (ties by priority), undated after, completed last |
| `assignee`, `completed`, `incomplete` | sorts by them | not orders iOS has: read as auto |
| `createdAt` | not an order web has: read as auto | newest first |
| `manual`, tasks the arrangement does not name / nothing arranged | oldest first | newest first |

`auto`, `completedAt`, an unknown order and an absent one agree and are fixture-locked. Web's
`when` is the one worth fixing soonest: on web that sort currently does nothing.

### D34 — a timed task due earlier today is overdue on web, not on iOS

Web's `isTaskOverdue` compares instants for a timed task, so 9am today is overdue at 10:30. iOS's
`applyListDueDateFilter` (and `filters::matches_due_date`) compares days: it is due today, and
overdue tomorrow. **Resolved 2026-10-03 toward iOS.** (`filters::is_overdue`, used for row
styling, compares instants like web — a separate rule, not part of the list filter.)

### D35 — `tomorrow`, `this_calendar_week`, `this_calendar_month` exist only on web

Web filters by them; iOS does not know them and, like any unknown value, keeps every dated task
and drops undated ones (D7). **Resolved 2026-10-03 toward iOS** — the core does the same. If the
windows are wanted on every client, they are added on iOS and here together, from web's
`date-filter-utils.ts` (whose answers this fixture already records).

### D36 — two edges of the recently-completed window

- **"Since the Nth" when last month has no Nth.** On 9 March, "since the 31st": web's
  `setMonth(-1)` then `setDate(31)` rolls on to **3 March**; iOS's `Calendar` arithmetic and this
  crate clamp to **28 February**. Resolved 2026-10-03 toward iOS. (One more iOS corner, recorded
  not followed: when *this* month lacks the Nth — 9 September, "since the 31st" — iOS builds 31
  September, which Foundation reads as 1 October, and steps back to **1 September**; web and this
  crate say 31 August, the date the setting names. That iOS answer is an accident of lenient date
  building rather than a choice, and the fixture keeps web's answer for it. **Decided 2026-10-03
  (Jon): 31 August.** iOS was changed to match — `getRecentlyCompletedCutoff` clamps the day to
  last month's length, `RecentlyCompletedWindowTests.testCutoff_sinceDayOfMonth_D36_*`.)
- **An unreadable `since-date`.** Web's cutoff becomes an invalid date and hides every completed
  task; iOS (`?? now`) and this crate count from now, which only differs for a completion stamped
  in the future. Resolved toward iOS.

### D37 — web caps a reminder at five snoozes

`lib/reminder-snooze.ts` refuses a sixth snooze of one server reminder. iOS snoozes as often as
asked (and, per D24, moves the due date rather than a queue row), and so does this crate. Both
agree on when a snooze comes back — now plus the minutes, within the v1 route's 1..10080 — which
`reminders.json` locks. Web's in-browser reminder manager decides nothing a client could share
(its check loop is a stub), so `reminders::due_now` stays covered by its unit tests.

### D38 — markdown: what `marked` + DOMPurify and this crate read differently

The web renders HTML through `marked` and its sanitiser; this crate parses with pulldown-cmark
into blocks, which the Apple apps draw (`CoreRules.markdown`) and Windows will. `markdown.json`
compares at the level a reader sees — the web's sanitised HTML read back into blocks in a jsdom
window (`contracts/drivers/markdown.mjs` says exactly how). Over 107 texts covering what the apps
use, 103 agreed at first (130 texts, all agreeing, since AWTD-1064). **Fixed here** on the way (they were bugs, not choices):

- an address the browser cannot parse (`https://google.com]`, task 11cfaf6d's own description) was
  a link here and is plain text on the page;
- `http://www.` was upgraded to `https://` only for a bare host, not a typed link;
- a reference inside a code span showed its private-use placeholder;
- `<script>` / `<style>` content was shown as text;
- an indented code block lacked the closing newline `marked` gives every block.

**Resolved 2026-10-03 toward iOS; web adopted all three in AWTD-1064.** astrid-web's renderer
(`lib/markdown.ts`, still `marked` + DOMPurify in the browser, held to this crate by
`tests/lib/core-rules-markdown-parity.test.ts`) now keeps an ordered list's `start`, leaves a
reference inside code as typed, and reads typed HTML as its text. `markdown.json` has no disputed
cases, and 23 `xss-*` cases were added. The web's parity test checks their blocks against this
crate and the HTML against the sanitiser's allowlist. The rules door's `renderMarkdown` takes the
reader's `identifiers` (project keys) so a task id links through it as through
`render_with_identifiers`.

What was disputed (the core is what iOS draws, so it followed iOS):

- **An ordered list's start number.** `3. three` is numbered 3 here; on the web it is 1, because
  the sanitiser's attribute allowlist (`RICH_TEXT_ATTRS`) has no `start`. The fix is web's: add it.
- **A reference inside code.** Web draws the pill inside the code span or block; this crate shows
  the reference as typed, since code is literal.
- **Inline HTML the web allowlists** (`<strong>`, `<em>`, `<del>`, `<code>`, `<br>`): web formats;
  this crate reads all HTML as its text.

### D39 — Google Tasks: a deleted remote item is never adopted by title

Both passes (Apple's `GoogleTasksSyncService`, this crate's `services::external`, AWTD2-56) adopt
an unlinked Google item of the same title before creating a twin for a local task. Apple searches
the complete listing as the proxy returns it, which includes **deleted** items
(`showDeleted=true`, `metadata.deleted: "1"`). Adopting one links the local task to a deleted
twin, and the next pass's absence deletion — which treats a deleted item as absent — deletes the
local task. Somebody who once deleted "Buy milk" in Google loses the "Buy milk" they add here.

**Fixed here, not followed:** this is a data-loss bug, not disputed behaviour. The crate skips
deleted and tombstoned items when adopting (test:
`awtd2_56_a_deleted_remote_item_is_not_adopted`). iOS should add the same guard to both adopt
sites in `GoogleTasksSyncService` (the linked-list push and the My Tasks push) until it moves onto
this pass.


### D40 — a list that has never been given a sort: iOS arranges it by hand, web sorts it auto

`sortBy` is nullable on the server, and most lists never set it. iOS draws such a list as
`manual` (`TaskListView`: `selectedList.sortBy ?? "manual"`), which with nothing arranged is newest
first; web (`list.sortBy || "auto"`), the Mac and this crate read the absence as `auto`.
**Resolved 2026-10-03 toward iOS** (AITD-460): `rowsForList` reads an absent sort as `manual`, and
answers `sortBy: "manual"` for it. `filters::sort_by_setting` itself is unchanged — `None` there is
still auto, so the fixture's rule for an absent order stands; the list's absence is decided before
it. The Mac draws through `rowsForList` and moves with it. Test:
`aitd460_a_list_with_no_sort_is_newest_first_as_on_ios`.

### D41 — which subtasks a list splices, and from where

iOS (`TaskListView.computeFilteredTasks`, the shared `spliceSubtasks`) splices a parent's
subtasks from **every task it holds**, showing each by the view's **completion filter alone** — a
list filtered to priority 3 still shows a priority-0 subtask under a priority-3 parent: the
filters chose which tasks lead a row, not which of their parts are listed. This crate spliced
only from the list's own members, and only the subtasks that passed every filter, so a subtask
never filed in its parent's list, or of another priority, assignee or due date, vanished. The Mac
showed completed subtasks only under `all` / `completed`. **Resolved 2026-10-03 toward iOS**
(AITD-460): `rowsForList` splices from every cached subtask (`Store::subtasks`) by
`filters::shown_by_completion`. iOS also splices search results' subtasks under them: `searchTasks`
does so when asked (`subtaskDisplay`, `showSubtasks`); unasked it stays flat, as the Mac and web
draw search. Tests: `aitd460_a_subtask_shows_by_the_completion_filter_alone`,
`aitd460_a_subtask_outside_the_list_is_spliced_under_its_parent`,
`aitd460_search_splices_subtasks_when_asked`.

### D42 — ties in a sort fall in iOS's store order

Every list sort is stable, so tasks it cannot tell apart — the same priority and no due date under
`priority`, two subtasks made in the same second — keep the order they were read in. iOS reads its
store in `TaskOrdering` order (due date, undated last; then newest created; then id); this crate
read the cache in storage order, and ordered tied subtasks by id. **Resolved 2026-10-03 toward
iOS** (AITD-460): `rowsForList` and `searchTasks` read in `filters::display_order` first, and the
splice orders children by creation alone, stably. Test: `aitd460_ties_fall_in_ios_store_order`.

### What a shell may tell `rowsForList` (AITD-460)

Not a divergence, but the reason the Apple apps can draw from it without moving a pixel: some of a
list's inputs live in the shell ahead of the cache — the signed-in user (the apps sign in
themselves), My Tasks' filters (written to the account 300 ms after the last change), the
account's subtask display, a sort chosen on one device for a view with no list row (the Mac's sort
menu over My Tasks), and a public list the reader is not in, whose tasks never enter the cache.
`rowsForList` takes each (`RowsInputs`: `currentUserId`, `myTasks`, `subtaskDisplay`, `sortBy`,
`list`, `tasks`) and uses it over the cached one; `idsOnly` answers the ordered ids without
projecting rows, and `matched` counts what the filters kept, subtasks included. `listCounts`
answers that count for many lists in one read — the sidebar's saved-filter badges.

## The board and the pickers, checked against iOS (AITD-461, 2026-10-03)

The Apple apps drew the board and filled their pickers from Swift copies (`ProjectStatus.swift`,
`ProjectStateMove`, `MacBoardMove`, `AssigneeOptions`, `DueDateQuickPicks`, the list pickers'
filters). Those were run against this crate before they were deleted; every difference below was
**resolved toward iOS** (Jon, 2026-10-03: on disagreement follow iOS), and the Mac now draws
through the same commands.

### D43 — which cards a board column holds, and in what order

iOS drew a column from the project's **top-level** tasks, in the opened list's **manual order**
(cards the order never named last, in its store order), with **Done holding recent work only**: the
opened list's completion filter and recently-completed window (`boardColumnTasksSorted`). This crate
drew every domain task, subtasks included, in cache order, every finished task forever, and at
most 50 cards a column. **Resolved toward iOS**: `board` answers through `board_cards::cards` and
`board_cards::column_cards`. Done reads the filter as iOS's board does — absent or `default` is the
window, `show` / `all` (and anything else) everything, `hide` nothing — and times a card by
`updatedAt`, never `completedAt`, because iOS hands its window no completion time
(`board_cards::done_shows`; not the list view's D22 reading). `idsOnly` answers every card's id;
rows still come 50 a column unless `limit` says otherwise — a transport window for a shell that
draws the core's rows, not a limit on the board. A shell that has the project but no list of it may
ask by `projectId`. Tests: `aitd461_cards_are_top_level_in_the_manual_order_and_uncapped`,
`aitd461_done_holds_what_the_window_lets_through`, `aitd461_a_board_can_be_asked_for_by_project`,
`board_cards::tests`.

A drop is `dropBoardCard`: the move, then the card's new place in the opened list's manual order
(just above the card it was dropped on, or after the column's last card) with the list's sort set
to manual — what iOS's drop wrote (`resolveBoardReorder` + `updateListOnServer`). Unlike a move, a
drop onto the card's own column rearranges it. iOS placed the card against its column computed
without the board's custom states and with subtasks counted; the slot it was given is the one the
board drew, so this places it against the drawn column. Test:
`aitd461_a_drop_writes_the_move_and_the_manual_order`.

### D44 — a cached status row names its default column

iOS (`getProjectBoardColumns`) named a default column by a rename stored on the board, else by a
`listType: "status"` row it still had cached for that role, else by the default; the row's
`statusDescription` (or description) was the column's. This crate never read the rows. **Resolved
toward iOS**: `board_cards::columns_with_rows`, for the board and the status menu. Only the name
and description — the id is the role whatever is cached. Legacy Inbox and Done rows are ignored;
when two rows claim a role the last in iOS's order (`statusOrder`, then name) wins. Test:
`aitd461_a_cached_status_row_names_its_default_column`.

### D45 — a move to the card's own column, and leaving Done

iOS (`planProjectColumnMove`) did nothing when a card was moved to the column it was in, and
leaving Done **un-completed first**, then set the column; this crate wrote an edit of nothing, and
un-completed last. **Resolved toward iOS** for `moveTaskToColumn` and `setTaskStatus`; going to
Done still sets the column first, then completes through the completion service. Tests:
`aitd461_a_move_to_its_own_column_writes_nothing`, `aitd461_leaving_done_uncompletes_first`,
`aitd461_going_to_done_moves_then_completes`.

### D46 — the status menu never offers Done

iOS's state picker (`ProjectStatePicker`, task 7574067b) left Done out: it sits beside an explicit
Complete, and the chip was the copy of that action that never said it would finish the task.
`taskStatusOptions` offered it. **Resolved toward iOS**: the columns leave Done out; `current`
still names Done for a finished task. Test: `aitd461_the_status_menu_has_no_done`. A shell that
drew a Done chip from this answer loses it.

### D47 — who the assignee picker offers

Five differences from iOS's `AssigneeOptions.build`, all **resolved toward iOS** in
`rows::assignee::options`:

1. A list member whose user never hydrated is left out (this crate offered a minimal record).
2. You are offered whenever the task's lists name nobody — a list cached before its roster was,
   offline (AITD-413) — not only when no list resolves.
3. Names sort as written (`name`, else email; capitals before lower case), not lower-cased.
4. Who holds the task now is offered only when a caller passes `current_assignee`. The pickers'
   `assigneeOptions` no longer does, as iOS's picker never did; quick-add's preview still does.
5. A shell's own inputs win: `assigneeOptions` takes `listIds`, `discovered` (people its search
   found), `agents` and `currentUser`, and needs no `taskId` — a task not created yet can be asked
   about. Each option carries the person's `user` record for a shell that draws its own avatar.

The unassigned row stays first: both Apple pickers draw one there. Tests: `rows::assignee`'s
`aitd461_*`, `aitd461_assignee_options_take_the_editors_inputs`. The Mac's own builder offered
unhydrated members and the current assignee; it follows iOS now.

### D48 — a quick date pick is an all-day date, and a time pick stays on the task's day

iOS's date picker (`InlineDatePicker.setQuickDate`, saved through `updateTask(when:)`) writes the
reader's calendar day at UTC midnight and makes the task all-day — a timed task picked onto
"Tomorrow" loses its time. This crate kept the time of day and anchored on the task's date; the
Mac kept the time too. **Resolved toward iOS**: each `dueDateOptions` date carries the all-day
instant and `isAllDay: true`. The all-day storage contract is unchanged (UTC midnight). A time pick
on an all-day task now lands on the task's calendar date — this crate read the local day of its UTC
midnight, the day before west of UTC, where iOS combined the hour with the UTC date. An editor's
unsaved date travels as `draft`. Tests: `aitd461_a_date_pick_is_an_all_day_date`,
`aitd461_a_time_pick_stays_on_an_all_day_tasks_date`. This supersedes D26's note that only the
all-day pick's storage matched.

### D49 — the repeat presets carry iOS's keys

`rows::repeat` named the presets `repeat.never` … `repeat.custom`, keys iOS's `Localizable.strings`
lacks; iOS names them `repeating.one_time_only`, `repeating.daily` … `repeating.custom`
(`Task.Repeating.displayName`, translated in twelve languages). **Resolved toward iOS**: the presets
and a simple repeat's one-part summary use iOS's keys. A custom pattern's parts keep their
`repeat.*` keys — iOS words that sentence in Swift (`CustomRepeatSummary`, English only) and has no
keys for its fragments. Windows resolves these keys from its own resources and must rename the six
when it takes this revision. Test: `aitd461_the_presets_are_named_with_ios_keys`.

### D50 — the list picker is a checklist

iOS's `InlineListsPicker` offers every list a task can be filed in, its own marked, in the
sidebar's order (`ListOrdering`: favourites, then name), matched anywhere without regard to case,
with no cap and no "create"; this crate's `listPicks` left the task's own lists out, sorted by name,
capped at ten and offered a create. **Resolved toward iOS** as a mode: `listPicks` with `asToggles`
answers `ListToggles`; without it, the editor shape Windows draws is unchanged. An editor's unsaved
lists travel as `listIds`. iOS matched with the locale's case folding; this crate lower-cases. Test:
`aitd461_list_picks_as_toggles`.
