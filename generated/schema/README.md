# The wire, as JSON Schema

Generated. **Do not edit** — `cargo xtask gen-schema` writes these from the Rust types that *are*
the wire, and `cargo xtask check-schema` (and the xtask crate's own test suite, and CI's contracts
job) fails when a committed copy is not what those types produce.

| File | Root | What it describes |
|---|---|---|
| `command.json` | `astrid_core::app::Command` | the request vocabulary: every command, every field, by its wire spelling |
| `rule.json` | `astrid_rules::rules::Rule` | the same for the pure-rules door |
| `wire.json` | `astrid_core::app::schema::WireModels` | the response envelope, the models, and the projections that have a type |

## Why this exists

Until task `9ef7edb4`, only the command **`kind`** names crossed the boundary checked. Everything
else — about 6,000 lines of request and response shapes across Windows' `Models.cs` and
`Commands.cs`, Android's `@Serializable` DTOs, the iOS facade and web's `lib/core-rules` — was
hand-written on each side and compared by nobody.

And the failure that produces is silent. `serde` **ignores an unknown request field**: rename
`assigneeId` in the core and every client goes on sending the old name, which arrives as `None` and
writes nothing. No error, no log line, a control that half works.

## What it does not cover, and why

`Response::value` is a `serde_json::Value` — one envelope for every command, with the payload
differing per command — so nothing about an answer can be derived from the envelope itself. What
`wire.json` holds instead is the models and the projections that carry a `Serialize` impl.

The **list and board rows are not in it.** Those are assembled by hand with `serde_json::json!` in
`crates/astrid-core/src/app/dispatch/` (`row_json.rs` is 663 lines of it), so there is no Rust type
to derive from. Giving them types is its own task — *"Move list-row assembly into astrid-rules;
return TaskRow projections from both doors"* — and each one that lands joins `WireModels`.

That gap is stated rather than papered over. A schema that quietly covered half the wire while
looking whole would be worse than none, because every client's gate would pass and mean nothing.

## Who reads them

- **A consumer that can call Rust** reads the schema from the pinned crate —
  `astrid_core::app::schema::command()` — not from these files, the same way its bindings gate
  already calls `command_kinds()` rather than parsing `command.rs`. One revision, one answer,
  nothing vendored and nothing to go stale. astrid-windows' `bindings_contract.rs` does this.
- **A consumer that cannot** generates from the files at build time — Android's `model/*.kt`. For
  those, `check-schema` is what makes the committed copy trustworthy.

## The `schema` feature

The derives are behind a `schema` feature on `astrid-rules` and `astrid-core`, **off by default**.
`astrid-rules-wasm` is built from this core and its byte count is a product requirement, and a
shipped client never asks for a schema — so a consumer's gate turns it on as a **dev** dependency,
which under cargo's resolver 2 keeps `schemars` out of the library it ships.
