# Cross-platform contracts, and where the clients disagree

Rules that must behave identically on web, iOS/Mac and Windows. The mechanism is in
[`contracts/README.md`](../contracts/README.md); this file records the **decisions and the known
divergences** — the things a generated fixture cannot tell you.

A divergence listed here is not a Windows bug to fix locally. Fixing one means changing every
client together, and the entry says which behaviour this crate currently follows and why.

---

## 1. Repeating-task rollover

Canonical: `astrid-web/types/repeating.ts` and `astrid-web/lib/repeating-task-handler.ts`.
Apple: asks this crate (`rules` → `completion` / `nextOccurrence`) since 2026-09-28.
Here: `astrid_core::repeating`.

### D1 — Whose calendar a step is taken on

**Settled 2026-09-28: all-day tasks on UTC's, timed tasks on the person's.** Every calendar step in
`astrid_core::repeating` takes a zone (`chrono_tz::Tz`) and is done on that zone's wall clock.

- **All-day tasks step in UTC.** An all-day date is stored as UTC midnight and names a day, so
  UTC is its calendar — exactly web's `setUTC*` arithmetic, and what every contract fixture checks.
- **Timed tasks step in the person's zone.** "Monthly at 2pm" is 2pm in March as well as in
  February, and "until the 15th" ends on their 15th. Stepping a timed task in UTC — what web does,
  because its server runs in UTC — moves it an hour at every daylight-saving change.

The Apple apps always stepped months and years in the device's calendar (`Calendar.current`);
they now ask this crate, which answers the same for timed tasks and correctly for all-day ones,
which the old Swift did not. Web and Windows should follow: web by passing the person's zone to
its server-side rollover, Windows by nothing more than taking this crate.

### D2 — Weekly patterns ignore their interval

**Reproduced deliberately.** `astrid_core::repeating::next_weekday_occurrence` ignores `interval`.

Both web and Swift ignore `interval` for weekly custom patterns: `getNextWeekdayOccurrence` takes the
interval and never reads it. So "every 2 weeks on Mon and Wed" advances every week on all three
clients. The stored pattern says one thing and every client does another.

Diverging here would be worse than the bug: this client would schedule occurrences the others do not
have, on a field the user believes is shared. It changes on web first.

### D3 — "Same weekday of the month" drops the time of day

**Reproduced deliberately — on the person's calendar.**

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

Apple's list filter (`RecentlyCompletedPresets.applyCompletionFilterWithWindow`) knows
`completed` (only completed tasks) and `incomplete` (only open ones), and treats `hide` or any
unknown value as `default`. This crate (`filters::recently_completed::should_show_completed`)
knows only `hide` and `default`, and keeps everything for any other value. The pickers differ
too: Apple offers default / all / completed / incomplete, this crate default / hide / all.

- **Where it bites:** a list saved on a Mac as "completed" shows everything on Windows; one saved
  on Windows as "hide" shows the recent completions on Apple.
- **To settle against the web** (`filterCompletion` in the web's list settings) before either side
  moves.

### D23 — a mention on the Mac is plain text

`MacAutocomplete.insert` writes `@label ` rather than the `@[Name](id)` reference iOS and this
crate (`parse::mentions::insert`) write, so a mention typed on the Mac is never a reference: no
pill, no notification.

### D24 — snooze moves a different field, by different amounts

Apple's snooze (`ReminderPresenter.snoozeTask`) moves `dueDateTime` and offers 15 minutes, a day
and a week, while the notification action snoozes 60 minutes. This crate's `reminders` moves
`reminderTime` and offers 10, 30, 60 and 1440 minutes. Snoozing a reminder should not move the
task's due date; this crate is right about the field, and the choices are to be settled against
the web's.

### D25 — My Tasks on iOS is only what is assigned to you

iOS's My Tasks (`TaskListView`, inline) keeps tasks assigned to the reader. The Mac
(`MacMyTasks.filter`) and this crate (`filters::my_tasks`) keep those **and unassigned tasks the
reader created**, and this crate also applies the view's `filterAssignee`.

### D26 — Apple's due-date quick picks cannot clear a date, and pick local midnight

`DueDateQuickPicks` offers four dates and no "No due date", and builds them in the local
calendar. This crate's `rows::due_picks` leads with "No due date" and stores all-day picks as UTC
midnight, as every all-day date is stored.

### D27 — priority labels are one step apart

Apple labels 3/2/1/0 as Highest / High / Medium / Low; this crate as high / medium / low / none,
which is the web's. The stored numbers agree; only the words differ.

### D28 — each Apple list picker applies half of "can a task be filed here"

`rows::list_picks::is_destination` excludes virtual lists and board-status lists. iOS's
`InlineListsPicker` excludes only virtual lists and the Mac's `MacListPicker` only status lists,
so each offers a destination the other refuses.

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
