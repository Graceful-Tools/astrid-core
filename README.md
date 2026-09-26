# astrid-core

The shared core of the [Astrid](https://astrid.cc) clients, in Rust: models, the API client, the
offline SQLite cache, the Outbox write journal, sync, and every rule that must read identically on
web, Apple and Windows — repeating-task rollover, permissions, the keyboard scheme, search and
smart-task parsing, the editing session, board columns, connections.

**Web app and API (canonical for shared behaviour):** https://github.com/Graceful-Tools/astrid-web
**Windows app (first consumer):** https://github.com/Graceful-Tools/astrid-windows
**iOS and Mac apps (migrating):** https://github.com/Graceful-Tools/astrid-ios

## How it fits

```
astrid-windows   app/Astrid.App (WinUI, C#)  ──  crates/astrid-ffi (C ABI)  ──┐
astrid-ios       Astrid App / Astrid Mac (Swift)  ──  UniFFI bindings (spike)  ──┤──  astrid-core
                                                                                 │
                                                        HTTPS /api/v1/*  ◄───────┘
                                                        astrid-web (astrid.cc)
```

The core has no platform dependency. A shell hands it three things through traits — a secure
store for the session, a clock, and reachability — and a cache directory, and drives it through
one door: JSON commands to `astrid_core::app::App::run_json`, answered with JSON. The shell renders
and dispatches; **it decides nothing**. That split was built once for Windows (which cannot
compile Swift) and is what lets the Apple apps retire their second copy of every rule.

## The rules

1. **Backend writes go through a service** — never the API client from a shell or a worker.
2. **Complete a task ONLY via `TaskService::complete_task`** — `update_task(completed: true)`
   skips repeat rollover.
3. **Next-occurrence math lives ONLY in `astrid_core::repeating`** — mirror changes into
   `astrid-web/types/repeating.ts` and `astrid-ios/.../RepeatingTaskHandler.swift` until the
   Apple apps consume this crate.
4. **API paths are `/api/v1/...` only.** The request guard refuses anything else.
5. **Preserve offline behaviour.** Task, list, comment, chat, attachment and account-settings
   writes journal through the Outbox. A write that is online-only on purpose says so in its
   service's doc comment.
6. **Shared behaviour changes on web first.** Anything covered by a fixture in
   `contracts/fixtures/` is a cross-repo change: web, then regenerate, then here, then the
   clients. The fixtures are generated, never hand-edited.
7. **Port from Swift test-first.** The Swift tests are the specification: write the Rust tests
   red, port green, refactor. Do not improve behaviour while porting — a divergence found on the
   way goes in [docs/CONTRACTS.md](./docs/CONTRACTS.md), not into the code.

## Quick start

```bash
cargo test --workspace          # the inner loop
cargo clippy --all-targets --all-features -- -D warnings
cargo xtask check-contracts     # fixtures against ../astrid-web (Node.js required)
node contracts/export-from-web.mjs --web ../astrid-web   # regenerate them
```

The toolchain is pinned in `rust-toolchain.toml`. The core builds and tests on Windows, macOS and
Linux; CI runs all three.

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

```
crates/astrid-core/src/
  model/        wire shapes; lenient decoding, because the server is permissive
  api/          the only place that speaks HTTP
  store/        the SQLite cache; the read path never waits on the network
  outbox/       the write path: idempotent, retrying, dependency-ordered, dead-lettering
  services/     the canonical control points — tasks, lists, comments, chat, connections, …
  sync/ realtime/   the delta pull and the live stream
  app/          the command layer, the one door a shell uses
  repeating/ permissions/ filters/ parse/ keyboard/ board.rs editing.rs rows/
                pure, fixture-locked rules and the projections a screen draws from
crates/xtask/   cargo xtask check-contracts
contracts/      the fixtures generated from astrid-web, and the exporter (see contracts/README.md)
docs/CONTRACTS.md   the decisions and known divergences between clients
```

## License

MIT. See [LICENSE](./LICENSE).
