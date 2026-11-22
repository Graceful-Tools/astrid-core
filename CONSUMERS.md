# Who pins this core, and at which revision

**Ten copies of this core exist, in six repositories**, each taken by **git revision** rather than
by a published version. That is deliberate — there is no crates.io release and the fixtures move
with the code — but it means the core has nine live versions in the world at any moment, and until
2026-11-02 nothing anywhere said what they were. The Windows pin was read as 96 commits behind in
the November platform assessment on the strength of a stale manifest nobody had looked at.

This file is that statement, and `cargo xtask check-pins` keeps it honest on every push and once a
week.

**Nine, not four, since 2026-11-12** (task 97285677). The cross-client review of 2026-11-11
([docs/CROSS_CLIENT_REVIEW_2026-11.md](./docs/CROSS_CLIENT_REVIEW_2026-11.md) §2.1) found four
copies that no row described — astrid-vscode's and astrid-code's vendored rules wasm, and the
vendored design tokens in astrid-web and astrid-ios — and a fifth, astrid-windows's vendored
tokens, landed after it was written. **A repository can hold two copies**: three of them do, taking
the rules or the whole core *and* vendoring the generated design tokens. So a consumer is identified
by `<checkout>/<copy>`, not by its repository.

| Consumer | Repository | Takes | Pinned in | Revision |
|---|---|---|---|---|
| `astrid-web/astrid-rules` | [astrid-web](https://github.com/Graceful-Tools/astrid-web) | `astrid-rules`, as WebAssembly | `packages/astrid-rules/REVISION` | `a718622ea1e110f3f0d0e28eefa4315fc7ff8e53` |
| `astrid-web/astrid-design` | [astrid-web](https://github.com/Graceful-Tools/astrid-web) | the generated design tokens | `packages/astrid-design/REVISION` | `2acba9417dc30ac5531418931f54526d09dfdd5e` |
| `astrid-web/strings` | [astrid-web](https://github.com/Graceful-Tools/astrid-web) | the generated string catalog (`strings/generated`, web output) | `lib/i18n/core-strings/REVISION` | `a24ae98eb58c5c29f43411c1303731d4e3d9b5df` |
| `astrid-windows/core` | [astrid-windows](https://github.com/Graceful-Tools/astrid-windows) | the whole core, through a C ABI | `crates/astrid-ffi/Cargo.toml` (`rev =`) | `d8c3e5beed5d321eae860f33e86bcc19e3af89f2` |
| `astrid-windows/design` | [astrid-windows](https://github.com/Graceful-Tools/astrid-windows) | the generated design tokens, hash-gated | `design/REVISION` | `2b5034870c7b5215e9ad61f424abc4319ed76078` |
| `astrid-ios/core` | [astrid-ios](https://github.com/Graceful-Tools/astrid-ios) | the whole core, through UniFFI (`crates/astrid-uniffi`) | `core/Cargo.toml` (`rev =`) | `4b7df692d1cac2e0cff150c23314079accdc3598` |
| `astrid-ios/design` | [astrid-ios](https://github.com/Graceful-Tools/astrid-ios) | the generated `AstridTokens.swift` | nothing records it — see below | `2acba9417dc30ac5531418931f54526d09dfdd5e` |
| `astrid-android/core` | [astrid-android](https://github.com/Graceful-Tools/astrid-android) | the whole core, through UniFFI (`crates/astrid-uniffi`) | `core/Cargo.toml` (`rev =`) | `d8c3e5beed5d321eae860f33e86bcc19e3af89f2` |
| `astrid-vscode/astrid-rules` | astrid-vscode (private) | `astrid-rules`, as WebAssembly | `vendor/astrid-rules/REVISION` | `4b7df692d1cac2e0cff150c23314079accdc3598` |
| `astrid-vscode/design` | astrid-vscode (private) | the generated VS Code themes and web tokens, hash-gated (ADEV-198) | `design/REVISION` | `59fa85dc92431ccaf55ca83cbbfb80ae3bfb4fe3` |
| `astrid-code/astrid-rules` | astrid-code (private) | `astrid-rules`, as WebAssembly | `extensions/astrid/dist/astrid-rules/REVISION` | `4b7df692d1cac2e0cff150c23314079accdc3598` |

**A pin only moves forward** (ACTD-2). `check-pins` compares each row with the same row in the
table as `main` has it, and fails a move to a revision that does not descend from the old one: an
agent with a stale core checkout can otherwise pin a consumer *behind* a change already landed and
pinned, and nothing else notices. To back out a regression on purpose, say so in this file:
`<!-- pin-rollback: <consumer> — <why> -->`. The check then reports the move as a deliberate
rollback instead of failing it.

**Full forty-character shas, never short ones.** A short sha is ambiguous as the history grows,
and `git merge-base --is-ancestor` on an abbreviation that has become ambiguous fails in a way
that reads like a bad pin.

**`astrid-ios/core` took the bump on 2026-11-15** (now `8de00507`), past the leading-action and
board-state answers (CONTRACTS D62, D63) that AITD-656 / AITD-622 / AITD-672 needed to draw rows
from `TaskRow` and delete `TaskLeadingControl` and `TaskDetailProjectStateRow`.

**astrid-ios is also waiting on the rest of its row rules** (AWTD2-216, AITD-622 / AITD-634). The
rule door now answers `detailLayout` (field order, `showsBoardState`, `showsTaskBlockers`),
`leadingPicker` (`pickerShowsProjectState`, `isSomeoneElsesTask`), `listChips` (with
`hiddenListIds`) and `dayOffset`, so the Apple detail views can delete their Swift copies without
reading the App door's `taskDetail`. Taking them is a pin bump past the commit that lands this, and
that bump is a client release decision.

**The table is the source.** `check-pins` reads the rows — the key, the manifest paths out of the
*Pinned in* cell, and the revision out of the last one. It holds no list of paths of its own, so
recording a tenth copy is a row here and nothing in `crates/xtask`. What the code still holds is
which rows must *exist*, so a row cannot be deleted by a bad merge and go unnoticed.

**`astrid-web/strings` is the only strings copy** (ADEV-162): web vendors the catalog's output, the
other clients harvest words or gate on the key list (strings/README.md → *Adoption*), so they hold
no copy to pin. A client that starts vendoring `strings/generated` adds its row in the same change.

**`astrid-android/design` has no row yet.** It vendors `AstridTokens.kt` and does not record the
revision it came from, so a row would have to be a guess. It is nevertheless checked:
`cargo xtask check-design` compares the vendored file by content (see below), which needs no row.
The row follows when the client keeps a `REVISION` beside its copy. astrid-vscode vendors the
generated workbench themes and `web/astrid-tokens.css` under `design/`, with a `design/REVISION`,
and is in `CLIENTS` (ADEV-198).

### `cargo xtask check-design` checks every client's copy (ADEV-162)

`check-design [client checkouts…]` compares what each of astrid-web, astrid-windows, astrid-ios,
astrid-android and astrid-vscode vendors with this repository's generated files, by content. The
client list is `CLIENTS` in `crates/xtask/src/design.rs`. How the revision is found, in order:

1. the client's `REVISION` file (web, Windows);
2. an `astrid-core-revision:` line in the first lines of a vendored file. Core's own generated
   files cannot carry it, because a file cannot name the commit that contains it, so it is written
   by whoever vendors the copy;
3. none recorded (iOS, Android today): each vendored file must equal this repository's at
   **some commit on `main`**. That catches a hand-edited copy and one generated on an unmerged
   branch, and it prints the commit the copy equals.

A client named on the command line that differs fails the check; one found beside this repository
is reported only, and a missing checkout is *unchecked*.

### `astrid-ios/design` is recorded but cannot be checked by `check-pins`

`AstridTokens.swift`'s header carries what generated it and a `body-sha256` of its contents — and a
content hash is not a revision. It answers *has this copy been edited?*, not *which commit was it
generated from*, so **nothing in
astrid-ios can confirm the revision above**. It was `a503a62`, the one the 2026-11-11 review read, until
2026-11-20, when the copy on astrid-ios `main` was hashed against `design/generated/apple/AstridTokens.swift`
at each candidate: it differs from `a503a62`'s and is byte-identical to `2acba94`'s, which is also
what `main` generates today (ADEV-155). That is a reading by hand, not a check. `check-pins` reports the row as *unchecked* rather than comparing a
sha256 against a git sha.

Two ways to close it, either of which this check already understands: astrid-ios keeps a `REVISION`
beside its vendored copy and the row names it, or `gen-design` writes an `astrid-core-revision:`
line into the generated header — `check-pins` reads that line today, so the day the generator emits
it the row starts being compared with no change here. The second changes every generated file's
header and every consumer's vendored hash with it, so it is a deliberate change of its own, not a
tidy-up.

## What the check enforces

`cargo xtask check-pins` runs in CI on every push and pull request, and again on the Monday
schedule. It fails when:

- a row is missing, so a tenth consumer cannot be added without being recorded here;
- a revision is not a full forty-character sha;
- a revision is not a commit in this repository — a typo, or a pin at a branch that was rebased
  away;
- a revision is not an **ancestor of `main`** — a pin at an abandoned branch, which is how a
  client ends up running code that was never reviewed;
- a consumer's own checkout is available, was named on the command line, and the manifest in it
  disagrees with the row above **in any way but one**: it pins a short sha, a commit that is not
  here, a commit that is not on `main`, or a commit *older* than the row (the row is ahead of its
  consumer).

**The one disagreement that does not fail is a row behind its consumer** (task ADEV-7): the
manifest pins a full sha that is on `main` and descends from the row. That is a client that bumped
before the table did, which is not a fault this repository can stop, and on 2026-11-15 failing it
turned core `main` red three times in one day (core PRs #61 and #67), stalling every open PR until
someone hand-edited a row. `check-pins` reports it as *ROW BEHIND* under *Reported, not failed*, and
two things catch the table up:

- **`cargo xtask follow-pins <checkouts>`** moves exactly those rows to the consumer's full sha and
  touches nothing else. It follows only checkouts named on the command line (a sibling beside this
  repository may hold a bump nobody pushed), and a second run changes nothing.
- **The `Consumers follow pins` workflow** (`.github/workflows/consumers-follow-pins.yml`) runs it
  daily at 07:17 UTC and on demand, against the same fresh checkouts and the same
  `CONSUMER_PINS_TOKEN` the `pins` job uses. When a row moved, it opens or updates one pull request
  from `chore/consumers-follow-pins` (one commit on today's `main`), dispatches CI on it, and leaves
  it for a merge-commit merge once green.

Three shapes of manifest are read, by the file the row names:

| File | Read as |
|---|---|
| `REVISION` (or any other plain file) | the sha alone |
| `Cargo.toml` | the `rev =` on the `astrid-core` git dependency, inline or sectioned |
| `*.swift` | an `astrid-core-revision:` line in the generated header, if there is one |

It also prints how far behind `main` each pin is, **in commits and in days**. Those answer different
questions: twenty commits behind is nothing if they landed this morning, and two commits behind can
be a client nobody has touched in a month. **Being behind is not a failure.** A client bumps when it
is ready to take the change, and a pin that lags by a few commits is the system working; the gate
exists so that a pin which lags by a hundred commits is *visible* rather than discovered in an
audit.

### The toolchain, reported for the consumers that build this core

`rust-toolchain.toml` pins an exact Rust version and `Cargo.toml`'s `rust-version` declares the same
one; the `msrv` job in CI fails if they disagree. A consumer building this core with an older Rust
is unsupported, and the two files saying one number is what makes that statement checkable rather
than aspirational.

So `check-pins` reads the `rust-toolchain` of every consumer that **builds** the core — the rows
whose manifest is a `Cargo.toml` — and reports the ones that differ. The 2026-11-11 review found all
three floating `rust-toolchain = "stable"`, which means whatever Rust the machine happened to
install. A copy that only vendors a `REVISION` and some generated output compiles no Rust of ours,
so its toolchain is not this file's business.

This is **reported, never failed**: it is a fact about another repository's file, and the core's gate
has no business going red over it.

### What a comparison needs, and when it is skipped

**Five of the six repositories are private**, and `secrets.GITHUB_TOKEN` is scoped to this
repository alone. So the `pins` job checks `astrid-windows`, `astrid-ios`, `astrid-android`,
`astrid-vscode` and `astrid-code` out with a **`CONSUMER_PINS_TOKEN`** secret — a token with read
access to all five — and those steps are skipped when it is not set. The token predates the two
Astrid Code repositories, so until its access is widened their rows report as unchecked in CI; that
is task `fd7cf5c6`.

`check-pins` reports an unobtainable row as **unchecked**, under *Reported, not failed*, and still
passes. That matters in two ordinary cases: a fork's pull request gets no secrets at all, and a gate
that goes red for a reason the contributor cannot fix is worse than the gap it closed; and the rows
are still checked for shape and history, which is what they were before task `4c891314`.

A row is only ever *failed* on a manifest that is present **and** was named on the command line.
On a development machine the siblings are found beside this repository without being named, and a
working copy there is routinely a few days behind its own remote — so a difference against one of
those is printed, with a note that it may just need a pull, and does not fail. CI clones fresh and
names every path, so there a difference is real. A path is matched to its **repository**, so naming
`../astrid-web` once serves both of that repository's rows.

The alternative — each client asserting its own pin against this file — has the same auth problem
pointing the other way, so it is not a cheaper route.

## Bumping a pin

The pin moves in the consumer, and then the row here moves with it. **A client's pin-bump procedure
updates this row in the same change** — astrid-ios, astrid-windows, astrid-android, and astrid-web's
`REVISION` files alike — so the table is never a commit behind. If it is forgotten, core CI stays
green (the row is only *behind its consumer*) and the daily `Consumers follow pins` workflow opens
the one-line PR; that is the safety net, not the procedure.

1. In the consumer, set the new revision (`crates/astrid-ffi/Cargo.toml` on Windows, then
   `cargo update -p astrid-core`; the `REVISION` file on web, in astrid-vscode and in astrid-code;
   the UniFFI build on Apple and Android; re-vendoring the generated tokens for a `design` row).
2. Run that repository's gate. On Windows a bump trips the bindings contract whenever the core has
   gained a command, which is expected and is what the contract is for.
3. Update this table — `cargo xtask follow-pins ../<consumer>` writes the row from the consumer's
   checkout — and run `cargo xtask check-pins`.
