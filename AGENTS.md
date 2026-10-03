# Codex — astrid-core operational adapter

*Local Codex workflow for Astrid's shared Rust core.*

**Repository:** https://github.com/Graceful-Tools/astrid-core
**Web app + API (separate repo, canonical for shared behaviour):** https://github.com/Graceful-Tools/astrid-web
**Windows app (consumes this crate):** https://github.com/Graceful-Tools/astrid-windows
**Apple apps (consume this crate through UniFFI, `astrid-ios/core`):** https://github.com/Graceful-Tools/astrid-ios

---

## Read README.md before writing code

**[README.md](./README.md) holds the rules** — the service layer, the Outbox, repeating tasks, the
`/api/v1` guard, and the cross-platform contracts. [docs/CONTRACTS.md](./docs/CONTRACTS.md) holds
the decisions and the known divergences between clients. This file holds only commands and
workflow.

### Critical rules (full detail in README.md)

1. **Backend writes go through a service** — never the API client from a caller.
2. **Complete a task ONLY via `TaskService::complete` (or `complete_as`).**
3. **Next-occurrence math lives ONLY in `astrid_core::repeating`.**
4. **API paths are `/api/v1/...` only.**
5. **Preserve offline behaviour.** Everything writes through the Outbox unless its service says why not.
6. **Bug fixes are TDD:** RED regression test naming the task id, then green, then the gate.
7. **Contracts are fixtures, not prose.** Web first, regenerate, then here, then the clients.
8. **Nothing platform-specific lives here.** A platform need arrives through a trait in
   `astrid_core::platform`, implemented by the shell. The Windows C ABI is astrid-windows's
   `astrid-ffi`; Apple bindings belong to astrid-ios.

---

## Quick start

```bash
cargo test --workspace                                   # the inner loop
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo xtask check-contracts                              # against ../astrid-web
```

## Quality gate

All four commands above, green, before pushing. CI runs them on Windows, macOS and Linux and
checks the fixtures against astrid-web's `main`.

## Test locations

| Type | Path |
|---|---|
| Unit tests | alongside the code, in `#[cfg(test)] mod tests` |
| Fixture-locked contract tests | `crates/astrid-core/tests/*_contract.rs` |
| The command layer's tests | `crates/astrid-core/src/app/dispatch/tests.rs` |

---

## How work is done here

**Port order for anything coming from Swift:** read the Swift tests first, write the Rust tests
(RED), then port the implementation (GREEN), then refactor. The Swift tests are the specification.
A divergence found on the way goes in `docs/CONTRACTS.md`, not into the code.

**A change here reaches a client only when that client bumps its pinned revision.** Say so in the
completion report: "on `main` of astrid-core at `<sha>`; astrid-windows pins `<older sha>`". If the
change is one a client must take, file the bump on that client's board.

**Per-task process** (canonical, cross-repo — see `astrid-web/docs/FIXALL_WORKFLOW.md`): post a
strategy comment, RED-GREEN-refactor with a task-id-linked regression test, run the gate, post a
completion report, then mark the task complete.

---

## Approvals

**Always ask before:** deleting files, and anything that changes a client's pinned revision on
its behalf.

**Autonomous:** code analysis, local builds and tests, implementation, local commits, documentation,
and pushing `main`. A push here builds nothing that reaches anyone.

---

*This file is for Codex. Claude Code reads [CLAUDE.md](./CLAUDE.md) (same content).*
