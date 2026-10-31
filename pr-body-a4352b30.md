Astrid task [a4352b30](https://astrid.cc/t/a4352b30-746e-4aa3-9c8d-61fe7e5dd298), worked from a `/fixall` run on astrid-windows.

iOS's **Report an Issue** carries Astrid's production Bugs & Feedback list id in `Constants.swift:151`, so a white-label build sends a partner's users to Astrid's feedback list — filing bugs onto someone else's board and reading other people's. Jon settled the decision on 2026-10-31: the list is **brand config**, not a core constant.

## Why runtime, not build-time

Brand config reaches a client two ways, and this takes the second:

- `NEXT_PUBLIC_BRAND_APP_URL_SCHEME` is **build-time** — the installer registers the scheme with the OS, so it is fixed before the app talks to any server. It lives on `app::Config`.
- `appName`, `wordmark`, `slogan`, `agentName` are **runtime**, in the `brand` block of `GET /api/v1/capabilities`.

A feedback list id is a **row in the deployment's own database**, so only the deployment can name it correctly. A compiled-in id is right only for whichever server the binary was built against — which is iOS's present bug relocated, not fixed. It also passes the `brand` block's own three tests (`app/api/v1/capabilities/route.ts:73-85`): text only, not a client-side trust boundary, not resolved once at launch from the client's own assets.

## The change

`AccountService::feedback_list_id() -> Result<Option<String>>` reads `brand.feedbackListId` out of the capabilities blob the core already caches. That blob had no typed string reader — `supports()` can only pull booleans out of it, and it searches every group, which is right for a bare boolean and wrong for a specific field of a specific block. Plus `Command::FeedbackListId` so a shell can dispatch it.

**`None` means this deployment has no in-app feedback list**, and the shell opens the web instead, as Android already does. That is the answer on every deployment running today, so it is the case worth getting right, and it is also what a white-label brand profile says by setting the value blank. A blank or whitespace id is not an id — it would have the shell open a list that cannot exist.

## Tests

Two, both naming the task:

- `a4352b30_the_feedback_list_is_the_deployments_own_or_there_is_none` — nothing before the first refresh, then the deployment's own id.
- `a4352b30_a_deployment_that_names_no_feedback_list_has_none` — six shapes that all mean none: no `brand` block, a `brand` block without the field, blank, whitespace, a non-string, and the key loose at the top level rather than inside `brand`.

## Shipping

The halves ship independently in any order — the field is optional on the wire and absence already means something correct. The web half is filed on the Astrid Web board; the iOS hardcode and the Windows UI are filed on theirs.

Gate: `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace` — all green.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
