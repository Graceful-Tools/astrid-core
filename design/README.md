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
cargo xtask gen-design                   # write design/generated from tokens.json and assets/
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
| `apple/AstridTokens.swift` | astrid-ios (iOS and Mac): SwiftUI values; brand colours stay `Brand.*` | `Astrid App/Utilities/AstridTokens.swift`, hash-checked by `check:contracts` (AITD-605/610) |
| `tokens.resolved.json` | every client: the source with each theme's `extends` flattened | `packages/astrid-design/` in astrid-web |

| `web/checkbox-art.json` | astrid-web: the checkbox's shapes as data, drawn inline so a task list loads no images | `packages/astrid-design/` |
| `apple/AstridDesign.xcassets` | astrid-ios (iOS and Mac): the checkbox layers (`checkbox-box`, `checkbox-repeat` as template images that take the priority tint, `checkbox-check` in its own colours) and the agent marks (`agent-*`), all vectors | a second asset catalog in each app target |
| `android/drawable/checkbox_*.xml` | astrid-android: the three layers as vector drawables; `box` and `repeat` are white, for a tint | `app/src/main/res/drawable/` |
| `windows/AstridCheckbox.xaml` | astrid-windows: a resource dictionary with one `ContentControl` style per layer; `box` and `repeat` take the control's `Foreground` | merged into `App.xaml` |

Android (Compose) and Windows (XAML) token emitters are added to `crates/xtask/src/design.rs` in the
change that adopts them, so each one is written against the project that compiles it. Until then
those clients can read `tokens.resolved.json` in a test.

Every generated source file starts with a `body-sha256` line: a hash of everything below it. A
consumer checks that hash in its own gate, with no access to this repository, so a hand-edited
copy fails there.

## Shared art

`assets/` holds the art every client draws, so none of them keeps its own copy (AWTD-1605):

| Path | What |
|---|---|
| `assets/checkbox/checkbox{,-checked,-repeat,-repeat-checked}.svg` | The task checkbox, four variants on a 56-unit canvas. The box and the repeat arrows are `currentColor`: the client sets the priority colour (`priority-*`), so the art follows the tokens and a partner's brand (DS21). The check is the `checkbox-check` token, `#89A912`, the same in every theme, over a black shadow at 85%, offset 2 and blurred 0.8 |
| `assets/agents/*.svg` | The agent marks astrid-web shows for each harness, as it shipped them |
| `assets/manifest.json` | Every file, its `sha256` (of its text with LF endings) and its owner: whose mark it is, or where the drawing came from |

The four checkbox variants are three layers, `<g id="box">`, `<g id="repeat">` and `<g id="check">`,
and the generator refuses a variant whose layers differ from the others', a box or arrows in a fixed
colour, a check in any green but the token's, and any SVG element other than `<path>`: Android and
Windows are given the paths, so what they cannot draw is refused rather than dropped. The shadow's
blur is drawn only on web; elsewhere it is unblurred, 0.8 of the canvas being under a pixel at the
sizes a checkbox is shown.

To change the art: edit the SVG, then `cargo xtask gen-design`, which writes the platform outputs and
the manifest's new hash. A new file is listed in the manifest with its owner first; the generator
refuses a file under `assets/` that the manifest does not list. A client vendors `assets/` at the
same revision as the tokens, and checks the hashes against the manifest in its own gate.

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
lacks, body text under 4.5:1 against the theme's own page (`bg-primary`, `bg-secondary`), an
ink (below) under 4.5:1 on the theme's surfaces or on its own wash, a label under 4.5:1 on the
accent, its hover or the error fill, a focus ring under 3:1 on the page, and a priority colour
under 3:1 on the card (bar the amber on a light card, DS22). A brand's accent is not a theme: it is
the `{brand.*}` references, which a partner build replaces in every theme at once.

## A meaning as a fill, and as text

`accent`, `error`, `success`, `warning` and `info` are one colour each, the same in every theme.
That is the colour of a **fill**, a border or a wash. As **text or an icon** on the theme's own
surfaces it does not read everywhere (the success green is 2.3:1 on white, the accent 2.84:1 on a
dark card), so each has an ink: `accent-ink`, `error-ink`, `success-ink`, `warning-ink`,
`info-ink`. A theme with a light appearance takes a darker shade, a dark one a lighter shade.

| Use | Token |
|---|---|
| A solid fill, a border, a wash (the colour at 12%) | `error` |
| A label, a link or an icon on a surface, or on the meaning's own wash | `error-ink` |
| Text on the solid fill | `accent-text` |
| The focus ring, and the line around a selected row | `border-focus`, `bg-selected-border` |

**A fill that carries a label is dark enough for it** (DS26). `accent`, `accent-hover` and `error`
are buttons, and `accent-text` (white) on each is 5.17:1, 6.70:1 and 4.83:1. The generator refuses
any of the three under 4.5:1. `success`, `warning` and `info` are not held to it: they are washes
and marks, and a label on one is the theme's text on its wash.

A fill that dark is a poor line on a dark theme (the accent is 2.84:1 on Dark's card), so the
lines and marks that carry meaning by themselves follow the theme: `border-focus` and `priority-low`
are the accent on a light theme and the accent's dark ink on a dark one, as is Dark's
`bg-selected-border`. The generator holds `border-focus` to 3:1 on the page (`bg-primary`,
`bg-secondary`) and every priority colour to 3:1 on the card, with one recorded exception: the
medium amber is 1.67:1 on a light card (DS22). Any other accent or error line on a dark surface (a
border, a ring a client draws from `accent`) should read the ink there.

The generator holds each ink to 4.5:1 on `bg-secondary`, on the card (`bg-hover` in a light theme,
`bg-secondary` in a dark one; DS9) and on its own 12% wash over the card. The page is not checked:
Ocean's is cyan, where an ink is 3.1-3.5:1, and a status colour belongs on a card there.

`accent-ink` is the brand's, like `accent`: `brand.accent-ink` for light themes and
`brand.accent-ink-dark` for dark ones, with Astrid's blues as the defaults. The generator has no
way to derive one colour from another, so a partner build supplies both with its accent (on web
`--brand-accent-ink-rgb` and `--brand-accent-ink-dark-rgb`, on Apple `Brand.accentInkColor` and
`Brand.accentInkDarkColor`, defined since AITD-615); the contrast check runs on Astrid's values only.
Deriving both inks from one accent, and checking a partner's, is AWTD2-159.

## Categories, the agent, and the inverse surface

Three kinds of colour that are not a status and not a surface of the theme (astrid-web AWTD-1612).

**The categorical palette** tells things apart where the colour means nothing else: the tile behind
a settings section's icon, a filter's icon, a webhook's kind. Eight hues, each a theme colour pair
read the same way as a meaning above: `category-<hue>` is the fill (one mid-tone in every theme, for
a wash, a dot or a tile), `category-<hue>-ink` the icon or label. The hues are `red`, `orange`,
`amber`, `green`, `teal`, `blue`, `purple` and `pink`; `CATEGORIES` in `crates/xtask/src/design.rs`
is the list. A client picks the hue nearest the colour it drew; a category that has a meaning (an
error, a success, the brand) uses that meaning's token instead.

**The agent ink** is the colour of an agent's name or icon, beside `agent-bubble` behind its
message: `agent` is its fill and `agent-ink` the text, held as an ink is, and also to 4.5:1 on
`agent-bubble`.

**`surface-inverse`** is a surface that is dark in every theme by design: a media viewer, a code
block, a landing page's footer. It is one colour everywhere (the generator refuses a theme that
restates it), and what is drawn on it is the dark theme's text and inks, so each dark theme's body
text is held to 4.5:1 on it. Web scopes such a panel to `.dark`, so every token inside it answers as
Dark.

The generator holds each category ink and the agent ink to what it holds a meaning's ink to: 4.5:1
on `bg-secondary`, on the card and on its own 12% wash. The fills are not held to a contrast: like
`success`, they are washes and marks.

| Token | Fill | Ink (Light, Ocean) | Ink (Dark) | Light: card / secondary / wash | Ocean: page (not held) | Dark: card / wash |
|---|---|---|---|---|---|---|
| `category-red` | `#EF4444` | `#B91C1C` | `#F87171` | 6.47 / 6.19 / 5.55 | 4.20 | 5.31 / 4.78 |
| `category-orange` | `#F97316` | `#C2410C` | `#FB923C` | 5.18 / 4.96 / 4.58 | 3.36 | 6.49 / 5.53 |
| `category-amber` | `#F59E0B` | `#B45309` | `#FBBF24` | 5.02 / 4.81 / 4.58 | 3.26 | 8.79 / 7.12 |
| `category-green` | `#22C55E` | `#15803D` | `#4ADE80` | 5.02 / 4.80 / 4.52 | 3.26 | 8.42 / 6.81 |
| `category-teal` | `#14B8A6` | `#0F766E` | `#2DD4BF` | 5.47 / 5.24 / 4.89 | 3.55 | 7.89 / 6.47 |
| `category-blue` | `#3B82F6` | `#1D4ED8` | `#60A5FA` | 6.70 / 6.41 / 5.84 | 4.35 | 5.77 / 4.95 |
| `category-purple` | `#A855F7` | `#9333EA` | `#C084FC` | 5.38 / 5.15 / 4.66 | 3.50 | 5.56 / 4.90 |
| `category-pink` | `#EC4899` | `#BE185D` | `#F472B6` | 6.04 / 5.78 / 5.21 | 3.92 | 5.54 / 4.88 |
| `agent` | `#A855F7` | `#9333EA` | `#C084FC` | 5.38 / 5.15 / 4.66; on `agent-bubble` 4.80 (Ocean 4.53) | 3.50 | 5.56 / 4.90; on `agent-bubble` 4.65 |

`surface-inverse` is `#111827`; Dark's primary, secondary and muted text on it are 17.74, 12.04 and
6.99:1.

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
| DS2 | Priority colours | iOS's `#6B7280`, accent, `#FBBF24`, `#EF4444` | Windows samples the checkbox art (`#B3B6BA`, `#328ACC`, `#F5AF39`, `#D5392E`). The art no longer has colours of its own to sample: `design/assets/checkbox` is `currentColor`, and `generated/windows/AstridCheckbox.xaml` draws it in the control's `Foreground` (AWTD-1605) | open on Windows until it draws the shared art in the priority tokens and drops `PriorityPalette`'s sampled values |
| DS3 | Surfaces and borders | iOS's | Windows follows the web's old values (light border `#EBEDF0`, selection `#F3F8FF`) | open on Windows; web adopts with its first vendored copy |
| DS4 | Leading control size, touch | 34 | Nobody: web draws 34 from the token (`size.leading-control`) | closed on web |
| DS5 | Selection outline | The Mac's (Jon, 2026-11-10): the accent at `state.selection-outline-opacity` (55%), 1.5 wide on pointer layouts and 2.5 on touch | iOS draws it solid. Web adopts with its next vendored copy | open on iOS (opacity) |
| DS6 | Repeat frame around a photo or the unassigned mark | The control's own size (`repeat-frame-scale` 1.0), and the photo or "U" fills it (`repeat-frame-inner-scale` 1.0): the arrows are the photo's border, in place of the priority outline (Jon, 2026-11-11, AITD-612 reopened: "The same size as the default profile photo"; was 0.625, and 1.25 before that) | iOS and Mac draw it by hand until they re-vendor at this revision. Web draws it as iOS does, the frame and the photo both at 1.0 from the tokens (AWTD-1579) | closed on web; Apple vendoring only |
| DS7 | Task details panel: opens out of the selected row over `motion.panel`, wearing the selection outline, with an arrow to the row. Wherever the list and the details share the screen: Mac, desktop web, iPad, Windows, and two-pane Android (tablets, foldables, dual-screen). Not a phone's single pane (Jon, 2026-11-10) | Mac and web | iPad, Android and Windows have neither | open on those three |
| DS8 | iOS resolves themes per view (fixed light values, a ternary at each use) | one palette per theme, as the Mac resolves them | iOS | open on iOS |
| DS9 | Web-only surfaces (sidebar, panel, pill, count badge, card glass) | not tokens yet | Web names each once in its theme stylesheets as a variable that reads a token, so none is a colour written out. Two directional shadows are still written out there (one upward, one to the left): `shadow` has no direction | closed as far as web can go; open: name the surfaces and a directional shadow here, with the component specs |
| DS10 | VS Code takes the editor's colours | product meanings only (priority order, overdue, glyphs) | astrid-vscode, by design | permanent |
| DS11 | Detail field icon column: field content starts under the title | 62 comfortable (16 + 34 + 12), 29 compact (19 + 10) | Web 58 (16 + 34 + 8): the control is the token's, the gap is 8 against 12 | open on web: the gap |
| DS12 | Subtask indent per depth, at most four deep | 16 | Web indents 24 | open on web |
| DS13 | Corner radius per component: row 8; board card 8 / 6; board column 12 / 8; details panel 16 / 14 | iOS / Mac | Web draws rows, cards and the panel at 12 | open on web |
| DS14 | Row minimum height | 76 comfortable, 52 compact | Web 64 at every width | open on web |
| DS15 | Motion comes from the tokens | `motion.*` | Web's stylesheets read them all; five durations are still written out in its components. iOS writes its springs out (board 0.28/0.88, panel 0.36/0.92, mode switch 200 ms against medium's 220). The Mac's hover is a literal 0.1 | closed for web's stylesheets, open for its components (five); open on iOS and Mac |
| DS16 | Completing a task | The Mac's and iOS's together (Jon, 2026-11-10): the checkbox pops to `state.complete-pop-scale` (1.28) on `motion.spring`, with a success haptic where the device has one | iOS has the haptic and no pop; the Mac has the pop; web and Android have neither until they adopt | open on iOS, Android, Windows; web adopts with its next vendored copy |
| DS17 | Reduced motion is honoured | every animation has a no-motion form | No client checks the system setting | open everywhere |
| DS18 | A completed task: title struck through and muted, its mark at `state.completed-mark-opacity` | iOS and Mac | Web also dims the whole row to 75% | open on web |
| DS19 | Title in a row | iOS's (Jon, 2026-11-10): it wraps without limit by default, and an appearance setting on every client chooses wrap, two lines or one line. The choice belongs to the machine, like the theme | The Mac stops at two lines with no setting | open on Mac, iOS, Android, Windows (the setting); web adopts now |
| DS20 | Hairline width | 0.5 | Web draws 1 px | open on web; 0.5 px only on high-density screens |
| DS21 | The checkbox art is fixed-colour images | priority colours, low being the brand accent (Dark: the accent's dark ink); the shared art in `design/assets/checkbox` takes them as `currentColor`, the check is `checkbox-check` (AWTD-1605) | iOS, Mac, Windows and Android still draw the PNGs, in the art's colours (`#328ACC` low, `#F5AF39` medium, `#D5392E` high, `#B3B6BA` none), so under a partner accent their square stays Astrid blue. Web draws the checkbox in the priority tokens (AWTD-1581), so low priority is the darker accent and follows a partner's brand | closed on web; open on iOS, Mac, Windows and Android until they draw the shared SVG (`generated/apple/AstridDesign.xcassets`, `android/drawable`, `windows/AstridCheckbox.xaml`) |
| DS22 | Priority glyphs as text (`!!` in a header or menu) | the priority colour | Medium's amber is 1.7:1 on white; the web used a darker orange (2.8:1) before adopting the token. "None" as text uses muted text on web. The generator holds each priority colour to 3:1 on the card and lists the amber on a light card (1.67:1 in Light and Ocean) as the one that fails; "none" passes on Dark's card at 3.04:1 | open: text variants of the priority colours, checked for contrast |
| DS23 | Trade-off grid and Decision cards: the five rating fills and their text colours, cells 84-150 wide with a 3 gap and radius 4, a 10 swatch in the key, cards at radius 10 with a 24 number badge | iOS (`DecisionViews.swift`) | Web draws a 4 gap, radius 6 cells and a 12 swatch. iOS draws the dark text as black at 87%, the token is `#111827` | open on web |
| DS24 | Markdown headings | not tokens yet | iOS sizes them with Dynamic Type; web writes 1.75 / 1.375 / 1.125 / 1 rem | open: heading sizes join `type` |
| DS25 | A meaning as text or an icon: `accent-ink`, `error-ink`, `success-ink`, `warning-ink`, `info-ink` | Web's (AWTD-1575): it wrote a darker shade for text on light and a lighter one on dark (`text-blue-600 dark:text-blue-400`), and reading the single mid-tone as text dropped links from 5.17:1 to 3.68:1 and error text from 4.83:1 to 3.76:1. Light and Ocean `#1D4ED8`, `#B91C1C`, `#15803D`, `#B45309`, `#0E7490`; Dark `#60A5FA`, `#F87171`, `#4ADE80`, `#FBBF24`, `#22D3EE`. The error ink is one step darker than web's `#DC2626`, which is 4.14:1 on its own wash; the accent ink was `#2563EB` until DS26 made that the fill, where it is 4.37:1 on its own wash | iOS, Mac, Windows and Android draw text and icons in the fill colour. Web adopts with its next vendored copy. A partner build has to supply `accent-ink` and `accent-ink-dark` with its accent | open on Windows and Android: adopt at the next re-vendor. Adopted on iOS and Mac (AITD-615, core `98596ed`): `Brand.accentInkColor` / `accentInkDarkColor`, error text and links read the inks |
| DS26 | A fill that carries a label: `accent`, `accent-hover`, `error` | Darkened (Jon, 2026-11-11: "Darken accent and error in the shared tokens, every platform follows"; web AWTD-1575) so the label on a button is readable. `accent` `#2563EB` (white on it 5.17:1, was `#3B82F6` at 3.68:1), `accent-hover` `#1D4ED8` (6.70:1, one step on so it is still a hover), `error` `#DC2626` (4.83:1, was `#EF4444` at 3.76:1), in every theme; the generator refuses a label under 4.5:1. What follows: a light theme's `priority-low` and `border-focus` are the accent; `accent-ink` on light themes moves to `#1D4ED8` (DS25); Dark's `border-focus`, `bg-selected-border` and `priority-low` become the accent's dark ink `#60A5FA`, because the new accent is 2.84:1 as a line on a dark card. `priority-low` joined them after web's browser pass found the repeat frame dim: the ink is 5.77:1 on Dark's card and 4.07:1 on a selected row, where the accent was 2.84:1 and 2.00:1; the other priority colours are the same in every theme. What stays: `priority-high` and `overdue` (`#EF4444`: priority's and the due date's own reds, drawn as marks and text, and darker would lose them on dark), the list palette's blue and red (a person's choice), `bg-selected` and light's `bg-selected-border` (tints) | Every client until it re-vendors. iOS and Mac also change `Brand.accentColor`, `Brand.accentHoverColor` and `Brand.accentInkColor`, which the generated Swift names and does not hold; a partner build supplies its own. A client that draws a border, ring or selection outline from `accent` on a dark theme should check it there | open on Windows and Android: adopt at the next re-vendor. Adopted on iOS and Mac (AITD-615): `Brand.accentColor` `#2563EB`, hover `#1D4ED8`, Dark's lines on the dark ink. Web adopts with its next vendored copy |
| DS27 | Categorical colours: a settings tile, a filter's icon, a webhook's kind | Eight hue pairs, `category-<hue>` and `category-<hue>-ink`, from the Tailwind shades web drew (500 for the fill; 700, or 600 for purple, as the light ink; 400 as the dark ink), each ink at 4.5:1 or better (web AWTD-1612) | iOS and Mac draw SwiftUI's system colours (`.orange`, `.purple`, `.pink`), which adapt to the appearance but are not checked for contrast. Web converts with its next vendored copy | open on iOS, Mac, Android and Windows |
| DS28 | An agent's name or icon | `agent` `#A855F7` and `agent-ink` (`#9333EA` light, `#C084FC` dark), readable on the card and on `agent-bubble` (web AWTD-1612) | iOS draws `.purple` (`ChatMessageBubble`, `ListMembershipTab`, the agent picker). Web drew `text-purple-500/600/400` | open on iOS, Mac, Android and Windows |
| DS29 | A surface that is dark in every theme (media viewer, code block, landing footer) | `surface-inverse` `#111827`, the same in every theme, with the dark theme's text on it (web AWTD-1612) | Web wrote `bg-gray-900` and scoped the viewer to `.dark` | open on the other clients where they draw one |
