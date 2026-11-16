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
cargo xtask check-deny          # supply-chain/deny.<repo>.toml is what deny.toml renders (gen-deny writes it)
cargo build -p astrid-rules --target wasm32-unknown-unknown   # the pure rules stay I/O-free
cargo deny check                # advisories, licences, wildcards, where the crates came from
# the fixtures are GENERATED in astrid-web: npm run export:contract-fixtures there, then copy
```

The toolchain is pinned in `rust-toolchain.toml`. The core builds and tests on Windows, macOS and
Linux; CI runs all three.

`cargo deny` is the dependency gate — the Rust half of astrid-web's `npm audit`. The rules are in
[`deny.toml`](./deny.toml); CI runs it on every push and PR and again every Monday, because an
advisory is published between commits. Install it with `cargo install cargo-deny --locked`;
`.github/dependabot.yml` proposes the bumps.

astrid-windows, astrid-ios and astrid-android each gate their own lockfile with their own
`deny.toml` — the four resolve much the same versions, so an advisory on the sign-in path is one
advisory in four places. **Those three files are GENERATED from this repository's** by
`cargo xtask gen-deny`, into `supply-chain/deny.<repo>.toml`, with `[sources] allow-git` the only
variance (task `32d7c04d`). That is not a shared config read across repositories: a consumer's copy
changes when it bumps its pin, like every other shared decision, so a licence allowed here does not
silently change anyone else's gate. `cargo xtask check-deny` and an xtask test assert the copies are
current; each consumer asserts its own against `astrid_core::supply_chain` at the revision it pins.

## Consuming it

A client pins a revision and bumps it deliberately, the same discipline as the fixtures:

```toml
[dependencies]
astrid-core = { git = "https://github.com/Graceful-Tools/astrid-core.git", rev = "<sha>" }
```

A shell's bindings are checked against `astrid_core::app::command_kinds()` in the shell's own gate:
every command the shell sends must be one the core has a variant for, or it is a button that does
nothing.

## The web binding's size is a budget

The browser has to **load** `astrid-rules-wasm`, so its compressed size is a product requirement.
CI's `wasm` job holds it to **290,000 brotli bytes** (`scripts/wasm-size.mjs`), measured on the
pre-`wasm-bindgen` `wasm`-profile artifact, which is what that job already builds. **The budget
ratchets down after each saving.** Raising it is allowed; doing either without saying what it
bought is not.

Measured on that artifact (task `4d05306d`):

| | raw | gzip | brotli | |
|---|---:|---:|---:|---|
| **budget +20 KB** | | | 269,757 | Budget 270,000 → 290,000 (Jon, 2026-11-15: *"raise the budget by 20kb"*). It buys headroom: the budget sat 243 bytes above the build, so each new rule on the door turned main's `wasm` job red and every core PR inherited it (PR #58, PR #62). The ratchet still applies: lower it again after the next saving |
| **today** | | | **269,757** | + the rules door's `waitingHandoff` and the returning holder on `waitingSettle` / `waitingWrite` (AWTD-1686: a handoff files the other half and the source wakes back to the agent that handed it off): **+1,744** (CI's measurement). Budget 269,000 → 270,000, 2026-11-16 |
| before the handoff | 2,025,151 | 409,141 | 268,013 | no second `TaskList`/`Task` decoder: `TaskList::new` and `nextOccurrence` stopped building values with `serde_json::from_value` (AWTD2-162 step 2, slice 1; −11,091 on top of the sidebar). Budget 279,800 → 269,000 (260,034 before the sidebar merged), 2026-11-15 |
| after the sidebar | 2,121,891 | 429,857 | ~279,363 | + the rules door's `sidebar` (AWTD-1681: the sidebar's Projects section, so web and the VS Code extension can draw the same sidebar as the native shells): **+8,192** (279,104 on macOS against 270,912), of which ~5 KB is decoding the request's `Vec<Project>` through serde's buffered `content` path and ~3 KB is serialising `Sections`. Budget 271,800 → 279,800, 2026-11-15 |
| after the row rules | 2,058,873 | 418,605 | 271,171 | + the rest of the row rules (AWTD2-216, PR #58), which merged without raising the budget and turned main's `wasm` job red at 271,171 (CI's measurement; 270,912 on macOS). Budget 269,500 → 271,800 in AWTD2-162's step-1 fix, 2026-11-15 |
| before that | | | 269,113 | + board rows, the callout and `listProjectMoves` (astrid-ios AITD-680: who may move a list into or out of a project, which web's server and client both ask); budget 268,000 → 269,500, 2026-11-15 |
| after task 4d05306d | 1,973,556 | 405,355 | 261,258 | no markdown renderer, 2026-11-15 |
| before that | 2,193,133 | 488,019 | 324,175 | 2026-11-15, at `108c99f` |
| with `opt-level = "s"`, as it was | 2,375,925 | 531,752 | 353,882 | 2026-11-14 |

**The browser build has no markdown renderer. The Node build does.** `astrid-rules-wasm` takes this
crate with `default-features = false`, which drops the `markdown` feature: `pulldown-cmark`,
`rows::chat`, `CommentRow::blocks` with `rows::comment::rows`, and the `renderMarkdown` rule.
`astrid-rules-wasm`'s own `markdown` feature turns it back on. `scripts/build-wasm.sh` builds
twice, `--features markdown` for `node/` and the default for `web/`, and then checks both packages
(`scripts/check-wasm-packages.mjs`): node must answer `renderMarkdown`, because astrid-web's server
renders through it and `tests/lib/core-rules-markdown-parity.test.ts` holds web's renderer to it,
and web must refuse it. The first cut fed both packages from one markdown-free artifact, and the
Node package lost the renderer that way (AWTD2-162). The web renders
markdown itself — that rule exists for the clients that do not — so it was downloading a parser in
order never to call it. **−62,917 brotli bytes, 19.4%**, which is three times `pulldown_cmark`'s
own 4% code share, because the Unicode tables and the `Block`/`Inline` serde code go with it.

**The browser build has no orchestration policy either** (AWTD-1685). The `orchestration` feature
is on for every native client and for the Node package (`astrid-rules-wasm/orchestration`), because
astrid-web's server asks it wherever an agent acts. No page asks it, so the browser artifact is
byte-for-byte what it was before the rules existed. `scripts/check-wasm-packages.mjs` asserts that
node answers `orchestrationMay` and web refuses it. See
[`docs/ORCHESTRATION_POLICY.md`](docs/ORCHESTRATION_POLICY.md).

What stays off that module is deliberate: `shows_text`, `is_empty`, `files_of` and
`renders_inline` are what `Rule::CommentVisibility` asks and none of them parses anything. Both
halves are asserted in `crates/astrid-rules/tests/browser_build.rs`, and CI clippies and tests the
combination directly — nothing else in the workspace compiles it.

**Where the bytes are now**, by `node scripts/wasm-size.mjs <file> --attribute` on a build with
`--config 'profile.wasm.strip=false'` — the code section, 993,771 bytes over 4,610 functions,
beside a 955,451-byte data section that is mostly chrono-tz's zone table:

| crate | | crate | |
|---|---:|---|---:|
| serde | 26% | chrono | 4% |
| astrid_rules | 20% | chrono_tz (code) | 3% |
| core | 18% | serde_core | 2% |
| serde_json | 16% | hashbrown | 2% |
| alloc | 8% | | |

(An unstripped build is larger and keeps its name section; it is a different artifact from the one
the budget measures, which is the point of the two commands being different.)

**chrono-tz is the obvious suspect and it is not the obstacle.** Rebuilt with its zone table
filtered down to UTC, the artifact loses 904,638 raw bytes (38%) but only **53,437 brotli bytes
(15%)** — a timezone table compresses almost away. Dropping it costs a cross-crate API change
(the caller would pass offsets from `Intl` instead of an IANA name) and still leaves ~300 KB —
~210 KB now that the renderer is out. It is step 3 of the plan below, after the cheaper two.

What the compressed size is actually made of is **code**, and the largest part of that is serde's
monomorphised deserializers: the biggest non-chrono-tz functions are `Task` and `TaskList`
`Deserialize` instantiations at 8–16 KB each, **each present twice** — once through
`serde_json::value` and once through `serde::private::de::content`, because `Rule` is an
internally tagged enum and serde buffers those. That is the door's wire format, not an oversight.

So a materially smaller browser build means compiling **less of the door**, not optimising it
harder — which is what the markdown feature does and what the rest of the plan does. The target
is **150,000 brotli bytes**, and the agreed order (Jon, 2026-11-15, task `4d05306d`) is one step
per PR, with the budget ratcheted after each:

1. ~~Drop markdown rendering from the browser build.~~ **Done** — 324,175 → 261,258.
2. **Feature-gate the door down to the rules the browser calls.** Design note:
   [`docs/BROWSER_DOOR.md`](docs/BROWSER_DOOR.md). Measured there: the internal `kind` tag costs
   only ~4 KB, so the wire format stays. What costs is how many of the 47 rules are compiled. A
   20-rule browser set measures **169,050**, and it drops chrono-tz's zone table without step 3's
   API change. Slice 1 (the two `from_value` constructors, −11,151) is done. The rule set is
   the open decision.
3. **Time zone offsets from `Intl` instead of `chrono-tz`.** ~53 KB, cross-crate API change.
   Worth doing after 2, not instead of it.

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
docs/ORCHESTRATION_POLICY.md   who takes a task, what an agent may do alone, who it asks (AWTD-1685)
docs/CROSS_CLIENT_REVIEW_2026-11.md   what each client still re-implements, theming, white-label gaps
```

## License

MIT. See [LICENSE](./LICENSE).
