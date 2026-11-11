# Design tokens

`tokens.json` is the one source for how Astrid looks: the colours of every theme, the spacing and
radius scales, shadows, type sizes, motion, the sizes of shared controls, the layout rules that
keep them aligned (`layout`) and how a control looks in each state (`state`). It is to appearance
what [`contracts/fixtures`](../contracts/README.md) is to behaviour. The system it belongs to —
the component specs and the rules for platform differences — is defined in astrid-web's
[`docs/specs/DESIGN_SYSTEM.md`](https://github.com/Graceful-Tools/astrid-web/blob/main/docs/specs/DESIGN_SYSTEM.md).

**The values are the iOS and Mac apps'** (`astrid-ios`, `Theme.swift` and the Mac's token files),
except where the register below records that another client was right.

```bash
cargo xtask gen-design                   # write design/generated from tokens.json
cargo xtask check-design [../astrid-web] # fail if generated is stale, or web's copy is not ours
```

`cargo test` runs the same staleness check, so a change to `tokens.json` that was not regenerated
fails the suite.

## Generated, never shipped

Nothing here is linked into the core. `astrid_rules::theme` decides which theme is chosen and says
plainly that it does not hand a shell hex values; that still holds. The generator writes each
platform's own kind of resource, the client vendors the file, and its toolkit does the theming.

| File in `generated/` | For | Vendored at |
|---|---|---|
| `web/astrid-tokens.css` | astrid-web: CSS custom properties, colours as `r, g, b` triplets | `packages/astrid-design/` with a `REVISION` |
| `apple/AstridTokens.swift` | astrid-ios (iOS and Mac): SwiftUI values; brand colours stay `Brand.*` | not adopted yet |
| `tokens.resolved.json` | every client: the source with each theme's `extends` flattened | `packages/astrid-design/` in astrid-web |

Android (Compose) and Windows (XAML) emitters are added to `crates/xtask/src/design.rs` in the
change that adopts them, so each one is written against the project that compiles it. Until then
those clients can read `tokens.resolved.json` in a test.

Every generated source file starts with a `body-sha256` line: a hash of everything below it. A
consumer checks that hash in its own gate, with no access to this repository, so a hand-edited
copy fails there.

## Changing a value

1. Change it in the iOS/Mac app first if it is a design change (the reference), or record here why
   another client's value is the right one.
2. Edit `tokens.json`, run `cargo xtask gen-design`, commit both.
3. Each client takes it by re-vendoring at the new revision. A value never changes in a client's
   copy.

## Adding a theme

A theme is data. To add one:

1. Add an entry under `themes` with its `appearance` (`light` or `dark`) and the theme it
   `extends`. Restate only the colours that differ.
2. Add the name to `astrid_rules::theme::Theme`, which is what lets somebody choose it.
3. `cargo xtask gen-design`.

The generator refuses a theme that the core cannot name, a colour name one theme has and another
lacks, body text under 4.5:1 against the theme's own page (`bg-primary`, `bg-secondary`), and an
ink (below) under 4.5:1 on the theme's surfaces or on its own wash. A brand's accent is not a
theme: it is the `{brand.*}` references, which a partner build replaces in every theme at once.

## A meaning as a fill, and as text

`accent`, `error`, `success`, `warning` and `info` are one mid-tone each, the same in every theme.
That is the colour of a **fill**, a border or a ring. As **text or an icon** on the theme's own
surfaces it is too light on white (the error red is 3.76:1) and was a lighter shade on dark, so
each has an ink: `accent-ink`, `error-ink`, `success-ink`, `warning-ink`, `info-ink`. A theme with
a light appearance takes a darker shade, a dark one a lighter shade.

| Use | Token |
|---|---|
| A solid fill, a border, a focus ring, a wash (the colour at 12%) | `error` |
| A label, a link or an icon on a surface, or on the meaning's own wash | `error-ink` |
| Text on the solid fill | `accent-text` |

The generator holds each ink to 4.5:1 on `bg-secondary`, on the card (`bg-hover` in a light theme,
`bg-secondary` in a dark one; DS9) and on its own 12% wash over the card. The page is not checked:
Ocean's is cyan, where an ink is 3.1-3.5:1, and a status colour belongs on a card there.

`accent-ink` is the brand's, like `accent`: `brand.accent-ink` for light themes and
`brand.accent-ink-dark` for dark ones, with Astrid's blues as the defaults. The generator has no
way to derive one colour from another, so a partner build supplies both with its accent (on web
`--brand-accent-ink-rgb` and `--brand-accent-ink-dark-rgb`, on Apple `Brand.accentInkColor` and
`Brand.accentInkDarkColor`); the contrast check runs on Astrid's values only.

## What a platform may do differently

Consistency is the default. A platform may **add** — a token it alone needs, a density (`compact`
for pointer-driven desktops, `comfortable` for touch) — and may use its own native control where
the toolkit provides one. It may not restate a shared value with a different number. Anything else
that differs is either a row below or a bug.

## Register

Where the clients differ from the tokens today. An entry leaves when the client adopts the value.

| # | What | Tokens say | Who differs | State |
|---|---|---|---|---|
| DS1 | Ocean text colours | Web's, one step darker than light's (web AWTD-1496: grey on cyan measured 3.1:1) | iOS, Mac, Android use light's | open on Apple and Android |
| DS2 | Priority colours | iOS's `#6B7280`, accent, `#FBBF24`, `#EF4444` | Windows samples the checkbox art (`#B3B6BA`, `#328ACC`, `#F5AF39`, `#D5392E`) | open on Windows |
| DS3 | Surfaces and borders | iOS's | Windows follows the web's old values (light border `#EBEDF0`, selection `#F3F8FF`) | open on Windows; web adopts with its first vendored copy |
| DS4 | Leading control size, touch | 34 | Web draws 32 px | open on web; a sizing change, so its own task |
| DS5 | Selection outline | The Mac's (Jon, 2026-11-10): the accent at `state.selection-outline-opacity` (55%), 1.5 wide on pointer layouts and 2.5 on touch | iOS draws it solid. Web adopts with its next vendored copy | open on iOS (opacity) |
| DS6 | Repeat frame around a photo or the unassigned mark | The control's own size (`repeat-frame-scale` 1.0), and the photo or "U" fills it (`repeat-frame-inner-scale` 1.0): the arrows are the photo's border, in place of the priority outline (Jon, 2026-11-11, AITD-612 reopened: "The same size as the default profile photo"; was 0.625, and 1.25 before that) | iOS and Mac draw it by hand until they re-vendor at this revision; web still draws a 20px photo in 32px arrows (AWTD-1576) and has to follow | open on web; Apple vendoring only |
| DS7 | Task details panel: opens out of the selected row over `motion.panel`, wearing the selection outline, with an arrow to the row. Wherever the list and the details share the screen: Mac, desktop web, iPad, Windows, and two-pane Android (tablets, foldables, dual-screen). Not a phone's single pane (Jon, 2026-11-10) | Mac and web | iPad, Android and Windows have neither | open on those three |
| DS8 | iOS resolves themes per view (fixed light values, a ternary at each use) | one palette per theme, as the Mac resolves them | iOS | open on iOS |
| DS9 | Web-only surfaces (sidebar, panel, pill, count badge, card glass) | not tokens yet | web keeps them in its theme stylesheets | open: tokenise with the component specs |
| DS10 | VS Code takes the editor's colours | product meanings only (priority order, overdue, glyphs) | astrid-vscode, by design | permanent |
| DS11 | Detail field icon column: field content starts under the title | 62 comfortable (16 + 34 + 12), 29 compact (19 + 10) | Web 56 (16 + 32 + 8), which follows from DS4 | open on web, with DS4 |
| DS12 | Subtask indent per depth, at most four deep | 16 | Web indents 24 | open on web |
| DS13 | Corner radius per component: row 8; board card 8 / 6; board column 12 / 8; details panel 16 / 14 | iOS / Mac | Web draws rows, cards and the panel at 12 | open on web |
| DS14 | Row minimum height | 76 comfortable, 52 compact | Web 64 at every width | open on web |
| DS15 | Motion comes from the tokens | `motion.*` | Web reads none of them (57 durations written out). iOS writes its springs out (board 0.28/0.88, panel 0.36/0.92, mode switch 200 ms against medium's 220). The Mac's hover is a literal 0.1 | open on all three |
| DS16 | Completing a task | The Mac's and iOS's together (Jon, 2026-11-10): the checkbox pops to `state.complete-pop-scale` (1.28) on `motion.spring`, with a success haptic where the device has one | iOS has the haptic and no pop; the Mac has the pop; web and Android have neither until they adopt | open on iOS, Android, Windows; web adopts with its next vendored copy |
| DS17 | Reduced motion is honoured | every animation has a no-motion form | No client checks the system setting | open everywhere |
| DS18 | A completed task: title struck through and muted, its mark at `state.completed-mark-opacity` | iOS and Mac | Web also dims the whole row to 75% | open on web |
| DS19 | Title in a row | iOS's (Jon, 2026-11-10): it wraps without limit by default, and an appearance setting on every client chooses wrap, two lines or one line. The choice belongs to the machine, like the theme | The Mac stops at two lines with no setting | open on Mac, iOS, Android, Windows (the setting); web adopts now |
| DS20 | Hairline width | 0.5 | Web draws 1 px | open on web; 0.5 px only on high-density screens |
| DS21 | The checkbox art is fixed-colour images | priority colours, low being the brand accent | Under a partner accent the square follows the brand and the checkbox art stays Astrid blue, on iOS and web alike | accepted as it is (Jon, 2026-11-10) |
| DS22 | Priority glyphs as text (`!!` in a header or menu) | the priority colour | Medium's amber is 1.7:1 on white; the web used a darker orange (2.8:1) before adopting the token. "None" as text uses muted text on web | open: text variants of the priority colours, checked for contrast |
| DS23 | Trade-off grid and Decision cards: the five rating fills and their text colours, cells 84-150 wide with a 3 gap and radius 4, a 10 swatch in the key, cards at radius 10 with a 24 number badge | iOS (`DecisionViews.swift`) | Web draws a 4 gap, radius 6 cells and a 12 swatch. iOS draws the dark text as black at 87%, the token is `#111827` | open on web |
| DS24 | Markdown headings | not tokens yet | iOS sizes them with Dynamic Type; web writes 1.75 / 1.375 / 1.125 / 1 rem | open: heading sizes join `type` |
| DS25 | A meaning as text or an icon: `accent-ink`, `error-ink`, `success-ink`, `warning-ink`, `info-ink` | Web's (AWTD-1575): it wrote a darker shade for text on light and a lighter one on dark (`text-blue-600 dark:text-blue-400`), and reading the single mid-tone as text dropped links from 5.17:1 to 3.68:1 and error text from 4.83:1 to 3.76:1. Light and Ocean `#2563EB`, `#B91C1C`, `#15803D`, `#B45309`, `#0E7490`; Dark `#60A5FA`, `#F87171`, `#4ADE80`, `#FBBF24`, `#22D3EE`. The error ink is one step darker than web's `#DC2626`, which is 4.14:1 on its own wash. Additive: no existing value changes | iOS, Mac, Windows and Android draw text and icons in the fill colour. Web adopts with its next vendored copy. A partner build has to supply `accent-ink` and `accent-ink-dark` with its accent | open on iOS, Mac, Windows, Android: adopt at the next re-vendor |
