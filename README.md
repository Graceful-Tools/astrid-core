# astrid-core

The shared core of the [Astrid](https://astrid.cc) clients, in Rust: models, the API client, the
offline SQLite cache, the Outbox write journal, sync, and every rule that must read identically on
web, Apple and Windows — repeating-task rollover, permissions, the keyboard scheme, search and
smart-task parsing, the editing session, board columns and the Waiting lane, connections.

Who uses it, and how:

| Repository | Uses | Through |
|---|---|---|
| [astrid-web](https://github.com/Graceful-Tools/astrid-web) | `astrid-rules` only (the pure rules), on its Node server | WebAssembly committed to `packages/astrid-rules`, pinned in its `REVISION` file |
| [astrid-ios](https://github.com/Graceful-Tools/astrid-ios) (iOS, iPadOS, Mac) | the whole core | UniFFI, through this repository's `crates/astrid-uniffi`, pinned by revision; see its `docs/CORE_MIGRATION.md` |
| [astrid-windows](https://github.com/Graceful-Tools/astrid-windows) (the first consumer) | the whole core | a C ABI (`crates/astrid-ffi`), pinned by revision |
| [astrid-android](https://github.com/Graceful-Tools/astrid-android) | the whole core | UniFFI (`core/`, moving onto `crates/astrid-uniffi`), pinned by revision |

astrid-web is also where the rules are **defined**: its TypeScript is canonical and generates the
fixtures this crate is tested against. The one exception is a case where web disagrees with iOS
(rule 6).

## How it fits

```
astrid-windows   app/Astrid.App (WinUI, C#)  ──  crates/astrid-ffi (C ABI)  ──────┐
astrid-ios       Astrid App / Astrid Mac (Swift)  ──  crates/astrid-uniffi (UniFFI) ┤
astrid-android   app/ (Kotlin, Compose)  ──  crates/astrid-uniffi (UniFFI)  ────────┤──  astrid-core
                                                                                   │
                                                        HTTPS /api/v1/*  ◄─────────┘
                                                        astrid-web (astrid.cc)
                                                          └─ runs astrid-rules as wasm (packages/astrid-rules)
                                                          └─ generates contracts/fixtures ──▶ here
```

The core has no platform dependency. A shell hands it two things through traits — a secure
store for the session and a clock — and a cache directory, and drives it through
one door: JSON commands to `astrid_core::app::App::run_json`, answered with JSON. The shell renders
and dispatches; **it decides nothing**. That split was built once for Windows (which cannot
compile Swift) and is what lets the Apple apps retire their second copy of every rule and let Android start without one.

## The rules

**These numbers are cited from doc comments throughout `crates/`, so they are stable: add at the
end, never renumber.** Until 2026-11-02 those comments cited `docs/ASTRID.md` §0 — astrid-windows's
numbering, from before the core was its own repository — and had already drifted apart from each
other: two sites called the `/api/v1`-only rule rule 5 and a third called it rule 6.

1. **Backend writes go through a service** — never the API client from a shell or a worker.
2. **Complete a task ONLY via `TaskService::complete`** (or `complete_as`) — `update_task(completed: true)`
   skips repeat rollover.
3. **Next-occurrence math lives ONLY in `astrid_core::repeating`** — a change starts in
   `astrid-web/types/repeating.ts` (rule 6); the clients take it by bumping their pinned revision.
4. **API paths are `/api/v1/...` only.** The request guard refuses anything else.
5. **Preserve offline behaviour.** Task, list, comment, chat, attachment and account-settings
   writes journal through the Outbox. A write that is online-only on purpose says so in its
   service's doc comment.
6. **Shared behaviour changes on web first; on disagreement, follow iOS.** Anything covered by a
   fixture in `contracts/fixtures/` is a cross-repo change: web, then regenerate, then here, then
   the clients. The fixtures are generated, never hand-edited. Where this crate and the iOS app
   disagree, this crate takes iOS's behaviour and web adopts it later (Jon, 2026-10-03; see
   [docs/CONTRACTS.md](./docs/CONTRACTS.md)). **The exception is a core-first rule** — the
   Waiting lane (`astrid_rules::waiting`, Jon 2026-11-12): canonical here from day one, its
   fixture generated here (`cargo xtask gen-core-contracts`) and copied by web, whose TypeScript
   mirrors it ([contracts/README.md](./contracts/README.md) "Core-first fixtures").
7. **Port from Swift test-first.** The Swift tests are the specification: write the Rust tests
   red, port green, refactor. Do not improve behaviour while porting — a divergence found on the
   way goes in [docs/CONTRACTS.md](./docs/CONTRACTS.md), not into the code.
8. **The shell decides nothing.** It renders what an answer names and dispatches commands back;
   every question with an answer — may this person do that, is this control a checkbox or a face,
   what does a new task inherit — is asked of the core. A `role == "admin"` in XAML or in Swift is
   the shape of bug this rule exists to make impossible. This is the "How it fits" split above,
   stated as a rule because doc comments need a number to cite.

## Design tokens

How Astrid looks has one source too: [`design/tokens.json`](./design/README.md), generated into
each client's own format by `cargo xtask gen-design`. The shared art — the checkbox and the agent
marks — lives beside it in [`design/assets`](./design/README.md#shared-art), with a manifest of
hashes and owners, and is generated the same way. Nothing from either is linked into the core.

## Quick start

```bash
cargo test --workspace          # the inner loop
cargo clippy --all-targets --all-features -- -D warnings
cargo xtask check-contracts     # our fixtures still say what ../astrid-web's do
cargo xtask check-core-contracts  # the core-first fixtures (waiting.json) are what the rules answer
cargo xtask check-design        # design/generated is what design/tokens.json and design/assets produce
cargo xtask check-schema        # generated/schema is what the wire types produce (gen-schema writes it)
cargo build -p astrid-rules --target wasm32-unknown-unknown   # the pure rules stay I/O-free
cargo deny check                # advisories, licences, wildcards, where the crates came from
# the fixtures are GENERATED in astrid-web: npm run export:contract-fixtures there, then copy
```

The toolchain is pinned in `rust-toolchain.toml`. The core builds and tests on Windows, macOS and
Linux; CI runs all three.

`cargo deny` is the dependency gate — the Rust half of astrid-web's `npm audit`. The rules are in
[`deny.toml`](./deny.toml); CI runs it on every push and PR and again every Monday, because an
advisory is published between commits. Install it with `cargo install cargo-deny --locked`;
`.github/dependabot.yml` proposes the bumps. astrid-windows and astrid-ios carry their own copies of
`deny.toml` over their own lockfiles — the three resolve the same versions today, so an advisory on
the sign-in path is one advisory in three places.

## Consuming it

A client pins a revision and bumps it deliberately, the same discipline as the fixtures:

```toml
[dependencies]
astrid-core = { git = "https://github.com/Graceful-Tools/astrid-core.git", rev = "<sha>" }
```

A shell's bindings are checked against `astrid_core::app::command_kinds()` in the shell's own gate:
every command the shell sends must be one the core has a variant for, or it is a button that does
nothing.

## Layout

Three crates, and the build tool. `astrid-rules` is the pure half — no clock, disk, network or entropy, so it also
builds for `wasm32-unknown-unknown` and the web can run the same rules (CI checks that build).
`astrid-core` is everything that waits on something, and re-exports every `astrid-rules` module at
its old path (`astrid_core::repeating`, `astrid_core::rules`, `astrid_core::model`, …), so a client
names the same items either way. `astrid-uniffi` is the binding layer every UniFFI shell builds —
Swift for the Apple apps, Kotlin for Android — one crate since 2026-11-12 rather than a copy in each
client (astrid-windows keeps its C ABI, `astrid-ffi`, beside its shell: UniFFI has no C# generator).

```
crates/astrid-rules/src/
  model/        wire shapes; lenient decoding, because the server is permissive
  repeating/ permissions/ filters/ parse/ keyboard/ board.rs editing.rs markdown.rs rows/ …
                pure, fixture-locked rules and the projections a screen draws from
  rules.rs      the stateless door: JSON in, JSON out, synchronous (`rules::run_json`)
  envelope.rs   the `{ ok, value?, error? }` answer both doors share
crates/astrid-rules/tests/   the fixture-locked contract tests
crates/astrid-rules-wasm/    `rules::run_json` for the web, via wasm-bindgen; `scripts/build-wasm.sh
                             <out>` writes the Node and browser packages astrid-web vendors
crates/astrid-uniffi/        the UniFFI doors (rules + client) and callbacks, for Swift and Kotlin
crates/astrid-core/src/
  api/          the only place that speaks HTTP
  store/        the SQLite cache; the read path never waits on the network
  outbox/       the write path: idempotent, retrying, dependency-ordered, dead-lettering
  services/     the canonical control points — tasks, lists, comments, chat, connections, …
  sync/ realtime/   the delta pull and the live stream
  app/          the command layer, the one door a shell uses
crates/xtask/   cargo xtask check-contracts
contracts/      the fixtures generated from astrid-web, and the exporter (see contracts/README.md)
docs/CONTRACTS.md   the decisions and known divergences between clients
docs/CROSS_CLIENT_REVIEW_2026-11.md   what each client still re-implements, theming, white-label gaps
```

## License

MIT. See [LICENSE](./LICENSE).
