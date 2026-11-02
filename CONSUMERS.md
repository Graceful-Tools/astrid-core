# Who pins this core, and at which revision

Four repositories consume `astrid-core`, each by **git revision** rather than by a published
version. That is deliberate — there is no crates.io release and the fixtures move with the code —
but it means the core has four live versions in the world at any moment, and until 2026-11-02
nothing anywhere said what they were. The Windows pin was read as 96 commits behind in the
November platform assessment on the strength of a stale manifest nobody had looked at.

This file is that statement, and `cargo xtask check-pins` keeps it honest on every push and once a
week.

| Repository | Takes | Pinned in | Revision |
|---|---|---|---|
| [astrid-web](https://github.com/Graceful-Tools/astrid-web) | `astrid-rules`, as WebAssembly | `packages/astrid-rules/REVISION` | `52a371d21781f59dd4814d68794f533df85587ae` |
| [astrid-windows](https://github.com/Graceful-Tools/astrid-windows) | the whole core, through a C ABI | `crates/astrid-ffi/Cargo.toml` (`rev =`) | `351c650e8047f13d7794bdb8084d38187c3504ed` |
| [astrid-ios](https://github.com/Graceful-Tools/astrid-ios) | the whole core, through UniFFI | `core/` (see its `docs/CORE_MIGRATION.md`) | `3677cfb631d6fe415db5a77403e2c79f4e0b8c2b` |
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
- an astrid-web checkout is available and its `packages/astrid-rules/REVISION` disagrees with the
  row above.

It also prints how far behind `main` each pin is. **Being behind is not a failure.** A client
bumps when it is ready to take the change, and a pin that lags by a few commits is the system
working; the gate exists so that a pin which lags by a hundred commits is *visible* rather than
discovered in an audit.

### What it cannot check from here, and why

The other three manifests live in **private** repositories. Reading them in CI needs a token with
cross-repository access, which this workflow does not have, so the `astrid-windows`, `astrid-ios`
and `astrid-android` rows are checked for shape and history but not compared against the
manifests they claim to describe. astrid-web is public and *is* compared.

Closing that gap is tracked as task `4c891314` on the Astrid Windows board: give the workflow a
`CONSUMER_PINS_TOKEN` secret and the check compares all four. The alternative — each client asserting its own pin against this
file — has the same auth problem pointing the other way.

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
