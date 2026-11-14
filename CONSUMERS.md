# Who pins this core, and at which revision

**Nine copies of this core exist, in six repositories**, each taken by **git revision** rather than
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
| `astrid-web/astrid-rules` | [astrid-web](https://github.com/Graceful-Tools/astrid-web) | `astrid-rules`, as WebAssembly | `packages/astrid-rules/REVISION` | `654e00421393753f2797ab4d3214eba5840530a0` |
| `astrid-web/astrid-design` | [astrid-web](https://github.com/Graceful-Tools/astrid-web) | the generated design tokens | `packages/astrid-design/REVISION` | `2acba9417dc30ac5531418931f54526d09dfdd5e` |
| `astrid-windows/core` | [astrid-windows](https://github.com/Graceful-Tools/astrid-windows) | the whole core, through a C ABI | `crates/astrid-ffi/Cargo.toml` (`rev =`) | `5b08fe01aece03ffc707dc2d7d11e70e38831c30` |
| `astrid-windows/design` | [astrid-windows](https://github.com/Graceful-Tools/astrid-windows) | the generated design tokens, hash-gated | `design/REVISION` | `1c2adfd7e51e6d4e6bab383777df8fba1a606592` |
| `astrid-ios/core` | [astrid-ios](https://github.com/Graceful-Tools/astrid-ios) | the whole core, through UniFFI (`crates/astrid-uniffi`) | `core/Cargo.toml` (`rev =`) | `9128b8a9d3da9ea0c022c358d661c8a31c0ffd42` |
| `astrid-ios/design` | [astrid-ios](https://github.com/Graceful-Tools/astrid-ios) | the generated `AstridTokens.swift` | nothing records it — see below | `a503a62039fca9a19ce7802a2d9f6d38ae5f89d8` |
| `astrid-android/core` | [astrid-android](https://github.com/Graceful-Tools/astrid-android) | the whole core, through UniFFI | `core/Cargo.toml` (`rev =`) | `1f61c8524194c3abc9f4fbe73a82194a98d0854d` |
| `astrid-vscode/astrid-rules` | astrid-vscode (private) | `astrid-rules`, as WebAssembly | `vendor/astrid-rules/REVISION` | `3dcc880a391dee13cce3aa7c0b09544a7b647ca2` |
| `astrid-code/astrid-rules` | astrid-code (private) | `astrid-rules`, as WebAssembly | `extensions/astrid/dist/astrid-rules/REVISION` | `765e38e3499546ba84d7666bdddbf63a326ec5e2` |

**Full forty-character shas, never short ones.** A short sha is ambiguous as the history grows,
and `git merge-base --is-ancestor` on an abbreviation that has become ambiguous fails in a way
that reads like a bad pin.

**The table is the source.** `check-pins` reads the rows — the key, the manifest paths out of the
*Pinned in* cell, and the revision out of the last one. It holds no list of paths of its own, so
recording a tenth copy is a row here and nothing in `crates/xtask`. What the code still holds is
which rows must *exist*, so a row cannot be deleted by a bad merge and go unnoticed.

### `astrid-ios/design` is recorded but cannot be checked

`AstridTokens.swift`'s header carries what generated it and a `body-sha256` of its contents — and a
content hash is not a revision. It answers *has this copy been edited?*, not *which commit was it
generated from*, so the revision above is the one the 2026-11-11 review read and **nothing in
astrid-ios can confirm it**. `check-pins` reports the row as *unchecked* rather than comparing a
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
  disagrees with the row above.

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

The pin moves in the consumer, and then the row here moves with it. In the same change, so the
table cannot be a commit behind:

1. In the consumer, set the new revision (`crates/astrid-ffi/Cargo.toml` on Windows, then
   `cargo update -p astrid-core`; the `REVISION` file on web, in astrid-vscode and in astrid-code;
   the UniFFI build on Apple and Android; re-vendoring the generated tokens for a `design` row).
2. Run that repository's gate. On Windows a bump trips the bindings contract whenever the core has
   gained a command, which is expected and is what the contract is for.
3. Update this table and run `cargo xtask check-pins`.
