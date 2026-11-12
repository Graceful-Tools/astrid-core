# Who pins this core, and at which revision

Four repositories consume `astrid-core`, each by **git revision** rather than by a published
version. That is deliberate — there is no crates.io release and the fixtures move with the code —
but it means the core has four live versions in the world at any moment, and until 2026-11-02
nothing anywhere said what they were. The Windows pin was read as 96 commits behind in the
November platform assessment on the strength of a stale manifest nobody had looked at.

This file is that statement, and `cargo xtask check-pins` keeps it honest on every push and once a
week.

> **Not yet rows (2026-11-11).** Two more repositories vendor the `astrid-rules` wasm and are not in
> the table or in `check-pins`: astrid-vscode (`vendor/astrid-rules/REVISION`, `3dcc880`) and
> astrid-code (`extensions/astrid/dist/astrid-rules/REVISION`, `765e38e`). Neither are the design-token
> copies (web `packages/astrid-design/REVISION`, iOS `AstridTokens.swift`). Adding them is AWTD2-160;
> see [docs/CROSS_CLIENT_REVIEW_2026-11.md](./docs/CROSS_CLIENT_REVIEW_2026-11.md) §2.1.

| Repository | Takes | Pinned in | Revision |
|---|---|---|---|
| [astrid-web](https://github.com/Graceful-Tools/astrid-web) | `astrid-rules`, as WebAssembly | `packages/astrid-rules/REVISION` | `98596ed9cc6927461e690c2b30ef00aaa3711439` |
| [astrid-windows](https://github.com/Graceful-Tools/astrid-windows) | the whole core, through a C ABI | `crates/astrid-ffi/Cargo.toml` (`rev =`) | `8c3274dba50a3760d7c38043ba8f446015523d21` |
| [astrid-ios](https://github.com/Graceful-Tools/astrid-ios) | the whole core, through UniFFI | `core/` (see its `docs/CORE_MIGRATION.md`) | `98596ed9cc6927461e690c2b30ef00aaa3711439` |
| [astrid-android](https://github.com/Graceful-Tools/astrid-android) | the whole core, through UniFFI | `core/` | `2cf2b5fb202ce99147d6a00c01e3041b1cdccf0a` |

**Full forty-character shas, never short ones.** A short sha is ambiguous as the history grows,
and `git merge-base --is-ancestor` on an abbreviation that has become ambiguous fails in a way
that reads like a bad pin.

## What the check enforces

`cargo xtask check-pins` runs in CI on every push and pull request, and again on the Monday
schedule. It fails when:

- a row is missing, so a fifth consumer cannot be added without being recorded here;
- a revision is not a full forty-character sha;
- a revision is not a commit in this repository — a typo, or a pin at a branch that was rebased
  away;
- a revision is not an **ancestor of `main`** — a pin at an abandoned branch, which is how a
  client ends up running code that was never reviewed;
- a consumer's own checkout is available, was named on the command line, and the manifest in it
  disagrees with the row above. All four are compared, not just astrid-web's:

  | Repository | Manifest read |
  |---|---|
  | astrid-web | `packages/astrid-rules/REVISION` — the sha alone |
  | astrid-windows | `crates/astrid-ffi/Cargo.toml` — the `rev =` on the `astrid-core` dependency |
  | astrid-ios | `core/Cargo.toml` — likewise |
  | astrid-android | `core/Cargo.toml` — likewise |

It also prints how far behind `main` each pin is. **Being behind is not a failure.** A client
bumps when it is ready to take the change, and a pin that lags by a few commits is the system
working; the gate exists so that a pin which lags by a hundred commits is *visible* rather than
discovered in an audit.

### What a comparison needs, and when it is skipped

**Three of the four manifests live in private repositories**, and `secrets.GITHUB_TOKEN` is scoped
to this repository alone. So the `pins` job checks `astrid-windows`, `astrid-ios` and
`astrid-android` out with a **`CONSUMER_PINS_TOKEN`** secret — a token with read access to all
three — and those three steps are skipped when it is not set.

`check-pins` then reports an unobtainable row as **unchecked**, under *Reported, not failed*, and
still passes. That matters in two ordinary cases: a fork's pull request gets no secrets at all, and
a gate that goes red for a reason the contributor cannot fix is worse than the gap it closed; and
the rows are still checked for shape and history, which is what they were before task `4c891314`.

A row is only ever *failed* on a manifest that is present **and** was named on the command line.
On a development machine the three siblings are found beside this repository without being named,
and a working copy there is routinely a few days behind its own remote — so a difference against
one of those is printed, with a note that it may just need a pull, and does not fail. CI clones
fresh and names every path, so there a difference is real.

The alternative — each client asserting its own pin against this file — has the same auth problem
pointing the other way, so it is not a cheaper route.

## Bumping a pin

The pin moves in the consumer, and then the row here moves with it. In the same change, so the
table cannot be a commit behind:

1. In the consumer, set the new revision (`crates/astrid-ffi/Cargo.toml` on Windows, then
   `cargo update -p astrid-core`; the `REVISION` file on web; the UniFFI build on Apple and
   Android).
2. Run that repository's gate. On Windows a bump trips the bindings contract whenever the core has
   gained a command, which is expected and is what the contract is for.
3. Update this table and run `cargo xtask check-pins`.

## The toolchain, while you are here

`rust-toolchain.toml` pins an exact Rust version and `Cargo.toml`'s `rust-version` declares the
same one; the `msrv` job in CI fails if they disagree. A consumer building this core with an older
Rust is unsupported, and the two files saying one number is what makes that statement checkable
rather than aspirational.
