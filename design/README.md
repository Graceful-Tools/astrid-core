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
cargo xtask check-design [../astrid-web …] # fail if generated is stale, or a named client's copy is not ours (CONSUMERS.md)
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
| `android/AstridTokens.kt` | astrid-android: Compose values, one `AstridPalette` per theme and a Material 3 `ColorScheme` built from it; brand colours are a parameter (`AstridBrand`) with Astrid's own as the default, so a partner build passes `BuildConfig`'s instead of regenerating | `app/src/main/java/com/astrid/design/AstridTokens.kt`, the package the file declares (AATD-85) |
| `windows/AstridTokens.xaml` | astrid-windows: a WinUI resource dictionary. Brushes `Astrid<Token>` in `Default` (the light values, which WinUI resolves when the system states no preference), `Light`, `Dark` and `HighContrast` (the system colours) theme dictionaries; the shell's own names (`AstridSurface`, `AstridSurfaceHover`) and WinUI's accent-button keys; radius, spacing (`AstridSpace<n>` as `x:Double`, `AstridInset<n>` as `Thickness`), the compact `layout.*` tokens (`AstridRowMinHeight`; `AstridRowRadius` as `CornerRadius`; `AstridRowPadding` as `Thickness`), motion and state. No `ocean`: WinUI has no third theme (AWTD-1699, ADEV-1) | `design/AstridTokens.xaml`, hash-checked; merged into `App.xaml` |
| `vscode/astrid-{light,dark,ocean}-color-theme.json` | astrid-vscode: one workbench colour theme per Astrid theme, contributed under `themes` as Astrid Light / Dark / Ocean (ids `astrid-*`). Each contributed file includes this one and adds VS Code's own Light+/Dark+ syntax colours, because there is no code palette here. Astrid Code defaults to them (ADEV-198) | `design/` with a `REVISION`, hash-checked; the wrappers are in the extension's `themes/` |
| `web/astrid-tokens.css` (again) | astrid-vscode's webviews: sizes, radii, layout and type at the compact density, and the product palette under an Astrid theme (ADEV-198) | `design/astrid-tokens.css`, same `REVISION` |
| `tokens.resolved.json` | every client: the source with each theme's `extends` flattened | `packages/astrid-design/` in astrid-web |

| `web/checkbox-art.json` | astrid-web: the checkbox's shapes as data, drawn inline so a task list loads no images | `packages/astrid-design/` |
| `apple/AstridDesign.xcassets` | astrid-ios (iOS and Mac): the checkbox layers (`checkbox-box`, `checkbox-repeat` as template images that take the priority tint, `checkbox-check` in its own colours) and the agent marks (`agent-*`), all vectors | a second asset catalog in each app target |
| `android/drawable/checkbox_*.xml` | astrid-android: the three layers as vector drawables; `box` and `repeat` are white, for a tint | `app/src/main/res/drawable/` |
| `windows/AstridCheckbox.xaml` | astrid-windows: a resource dictionary with one `ContentControl` style per layer; `box` and `repeat` take the control's `Foreground` | merged into `App.xaml` |

`AstridSurface` and `AstridSurfaceHover` are the `surface` and `surface-hover` tokens themselves
(DS9, ADEV-160); they were aliases of `bg-hover`/`bg-secondary` and `bg-tertiary` until the surfaces
were named. The Windows token dictionary carries a few more names that are not tokens, so that
adopting it deletes the shell's hand-written brushes rather than renaming their call sites (ADEV-1).
`AccentFillColorDefaultBrush`, `AccentFillColorSecondaryBrush` and
`TextOnAccentFillColorPrimaryBrush` are WinUI's own keys, set to `accent`, `accent-hover` and
`accent-text` because DS26 wants one accent fill in every theme where WinUI would derive a pale one;
they are left out of `HighContrast`, where WinUI's system-colour brushes should win. Both lists are
in `crates/xtask/src/design.rs` (`XAML_WINUI_KEYS`). WinUI's `SystemAccentColor`
ramp (`Light1..3`, `Dark1..3`) stays written out in the shell: its six steps are shades the register
does not hold, and the generator does not derive one colour from another (AWTD2-159).

Two things the emitters deliberately leave out. The Compose file declares a package, because a
Kotlin file in the default package cannot be imported at all; it is changed in `design.rs` and
re-vendored, never in the copy, which would break the `body-sha256` the Android gate checks. And a
VS Code theme carries no syntax colours: `tokens.json` has no code palette, so code stays
`editor.foreground` rather than the emitter inventing eight colours no other client has. A theme
file is also literals, so a partner's accent reaches the workbench by regenerating from the
partner's tokens — there is nothing to pass in at run time, as there is in Compose.

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
| DS1 | Ocean text colours | Web's, one step darker than light's (web AWTD-1496: grey on cyan measured 3.1:1) | Android read light's until AATD-85, and now reads the tokens. iOS reads Ocean's own text wherever a view takes it from `ThemePalette` (`palette.*`), but about 40 static `Theme.textPrimary` / `textSecondary` / `textMuted` reads still give light's (2026-11-20) | closed on the Mac (`MacTokensTests.testOceanTextOnMacIsTheTokenText`); partly open on iOS (the static reads); closed on Android (`OceanColors.textPrimary/textSecondary` are the token text; `DesignTokensTest` "Ocean's text is the darker text from the tokens (DS1)", AATD-85) |
| DS2 | Priority colours | iOS's `#6B7280`, accent, `#FBBF24`, `#EF4444` | Windows samples the checkbox art (`#B3B6BA`, `#328ACC`, `#F5AF39`, `#D5392E`). The art no longer has colours of its own to sample: `design/assets/checkbox` is `currentColor`, and `generated/windows/AstridCheckbox.xaml` draws it in the control's `Foreground` (AWTD-1605) | open on Windows until it draws the shared art in the priority tokens and drops `PriorityPalette`'s sampled values |
| DS3 | Surfaces and borders | iOS's | Nobody: web reads every surface and border from its vendored copy (`packages/astrid-design`, gated by `tests/styles/design-tokens.test.ts`). Windows followed web's old values (light border `#EBEDF0`, selection `#F3F8FF`) until task `1481f33a` took the tokens; its `DesignTokenTests` now asserts every surface, border and selection brush against the vendored copy in both themes, with no exception list | closed on web and Windows |
| DS4 | Leading control size, touch | 34 | Nobody: web draws 34 from the token (`size.leading-control`) | closed on web |
| DS5 | Selection outline | The Mac's (Jon, 2026-11-10): the accent at `state.selection-outline-opacity` (55%), 1.5 wide on pointer layouts and 2.5 on touch. **It is the chrome's selection, not every selection** (task `16392d99`): a list that navigates or configures wears the outline, while a row of content keeps its own selected state — `bg-selected` with `bg-selected-border` around it, the row line named above. DS26 settled that the day after this row was written, by keeping both of those tints among the values that stay and by measuring `priority-low` *on a selected row*, which only exists if a selected row is still filled. Windows' four selectable surfaces are the worked example: the sidebar's three lists, the settings cards and the command palette take `SidebarRowStyle`'s outline (tasks `787c7aa5`, `b37d76c3`), and the task rows tint (`16392d99`). **Seen, not only asserted** (2026-11-14): `SidebarSelectionTests` and `PaletteSelectionTests` render the app and read the pixels, so a returning selected background fails rather than quietly beating the outline | iOS draws it solid: `Color.accentColor` at a literal 2.5 (`iPadTaskManagerView.swift`, `DetailPanelChrome.swift`). Web reads the width and opacity from the tokens (`--selection-outline-*` in `styles/components.css`) | closed on web; open on iOS (opacity; astrid-ios ADEV-163) |
| DS6 | Repeat frame around a photo or the unassigned mark | The control's own size (`repeat-frame-scale` 1.0), and the photo or "U" fills it (`repeat-frame-inner-scale` 1.0): the arrows are the photo's border, in place of the priority outline (Jon, 2026-11-11, AITD-612 reopened: "The same size as the default profile photo"; was 0.625, and 1.25 before that) | iOS and Mac draw it by hand until they re-vendor at this revision. Web draws it as iOS does, the frame and the photo both at 1.0 from the tokens (AWTD-1579) | closed on web; Apple vendoring only |
| DS7 | Task details panel: opens out of the selected row over `motion.panel`, wearing the selection outline, with an arrow to the row. Wherever the list and the details share the screen: Mac, desktop web, iPad, Windows, and two-pane Android (tablets, foldables, dual-screen). Not a phone's single pane (Jon, 2026-11-10): a phone pushes the details instead (DS30) | Mac and web | iPad conforms (AITD-607 for rows; AITD-679 for board cards, DS32). Android and Windows have neither | open on Windows. Closed on Android (AATD-85 DS7/DS5 slice): `ui/detail/DetailsPanel.kt` and `ui/common/SelectionOutline.kt`, revealed over `motion.panel` in two-pane layouts |
| DS8 | iOS resolves themes per view (fixed light values, a ternary at each use) | one palette per theme, as the Mac resolves them | iOS: the ternaries are gone (AITD-611; its ratchet holds `darkSchemeTernaries` at 0), but 142 reads still pick one theme's value by name (`fixedThemePicks` in `scripts/design-metrics.budget.json`, 2026-11-20) | partly open on iOS: the fixed picks move to `ThemePalette` one view at a time |
| DS9 | Surfaces (card, sidebar, panel, comment, pill, count badge) and the details panel's shadow | tokens (ADEV-160) | `surface`, `surface-hover`, `surface-border`, `sidebar-bg`, `panel-bg`, `comment-bg`, `pill-bg`, `count-bg` are theme colours with web's per-theme choices as values. `shadow` gained a signed `x` (absent is 0; negative falls to the left), and `shadow.panel` is the Mac popout's `radius 16, x 2, y 4` (Decided, agent, 2026-11-20: the Mac's, the source of the panel design; its alpha is the iPad's 0.18, the one native opacity on record; a client on the panel's other edge mirrors `x`). | web's pin and its `--theme-*` definitions follow; card glass stays web's own until a component spec names it |
| DS10 | VS Code: whose colours | **Two products, two answers** (Jon, 2026-11-21, ADEV-198). The extension inside someone else's VS Code takes the editor's colours: its webviews read `--vscode-*`, and product meanings fall back to the nearest editor variable. **Astrid Code is an Astrid product**: it ships the three generated workbench themes and defaults to Astrid Light/Dark, following the OS. Under an Astrid theme the webviews take product meanings from the tokens (priority, overdue, `surface` / `surface-hover`, the count badge). In every theme they take sizes, radii, layout and type from the tokens at the compact density. A partner's extension ships no Astrid theme | astrid-vscode adopts at core `59fa85dc` (themes and `web/astrid-tokens.css` vendored under `design/`, hash-checked by `check-design`). Astrid Code defaults to them | closed |
| DS11 | Detail field icon column: field content starts under the title | 62 comfortable (16 + 34 + 12), 29 compact (19 + 10) | Nobody: web's field rows and title row share `layout.row-gap`, so content starts at 62 (AWTD-1617) | closed on web |
| DS12 | Subtask indent per depth, at most four deep | 16 | Nobody: web indents `layout.subtask-indent` per depth, four deep at most (AWTD-1617) | closed on web |
| DS13 | Corner radius per component: row 8; board card 8 / 6; board column 12 / 8; details panel 16 / 14 | iOS / Mac | Nobody: web reads each radius from `layout`, compact with a pointer and comfortable on touch (AWTD-1617). iOS and Mac draw the same numbers but type them out; nothing there reads `AstridTokens.Layout` (2026-11-20) | closed on web; iOS and Mac read the tokens with astrid-ios ADEV-163 |
| DS14 | Row minimum height | 76 comfortable, 52 compact | Nobody: web reads `layout.row-min-height`, compact with a pointer and comfortable on touch; it has no density setting (AWTD-1617) | closed on web |
| DS15 | Motion comes from the tokens | `motion.*` | Web reads them all: its ratchet holds written-out durations at 0 (`motionLiterals` in `tests/rules/design-metrics.budget.json`). iOS writes its springs out (board 0.28/0.88, panel 0.36/0.92, mode switch 200 ms against medium's 220). The Mac's hover is a literal 0.1 | closed on web; open on iOS and Mac (astrid-ios ADEV-163) |
| DS16 | Completing a task | The Mac's and iOS's together (Jon, 2026-11-10): the checkbox pops to `state.complete-pop-scale` (1.28) on `motion.spring`, with a success haptic where the device has one | iOS has the haptic and no pop; the Mac has the pop, with the scale typed out (`1.28` in `MacTaskRow.swift`); Android has neither. Web pops the checkbox to the token (`taskCheckboxPop` in `styles/components.css` reads `--astrid-state-complete-pop-scale`) | open on iOS (the pop); closed on web and Android (AATD-85 DS16 slice: `ui/common/CompletionPop.kt` pops to `state.complete-pop-scale` on `motion.spring`). Closed on Windows (task `787c7aa5`): `PriorityMark` pops the mark to `state.complete-pop-scale` on `motion.spring`, read out of the resources rather than retyped, and a desktop has no haptic to give. **Seen, not only asserted** (2026-11-14): `CompletionPopTests` completes a task and watches the mark through UI Automation — 64 → 69 → 64 over about 150 ms — so it gates that the pop runs *and* that the spring settles, which the resource assertions cannot |
| DS17 | Reduced motion is honoured | every animation has a no-motion form | Android and Windows don't check the system setting. iOS and Mac route their springs through `ReducedMotion` / `MacMotion`, held by the source contract `springs-honour-reduce-motion` (AITD-624); an ease still animates there. Web stops every transition and animation under `prefers-reduced-motion` (a spinner keeps turning), and its script scrolls jump (AWTD-1617) | closed on web; closed on iOS and Mac for springs; closed on Android (`ui/theme/ReducedMotion.kt` `LocalReducedMotion` reads the system setting; the panel, board fling, chat, presence and completion pop read it); open on Windows |
| DS18 | A completed task: title struck through and muted, its mark at `state.completed-mark-opacity` | iOS and Mac | Nobody: web no longer dims the row (AWTD-1617) | closed on web |
| DS19 | Title in a row | iOS's (Jon, 2026-11-10): it wraps without limit by default, and an appearance setting on every client chooses wrap, two lines or one line. The choice belongs to the machine, like the theme | The Mac stops at two lines with no setting. Web has the setting, "Task titles" in Appearance (astrid-web #701) | closed on web and Windows (task 3d5cca89: `title_display`, this machine's like the theme); closed on Android ("Task titles" in Appearance: wrap, two lines or one, `ui/theme/RowTitleLines.kt`, AATD-85 DS19 slice); open on Mac, iOS |
| DS20 | Hairline width | 0.5 | Nobody: web's `border` is the token on screens of 2dppx and up, 1 px below, where half a pixel can't be drawn (AWTD-1617) | closed on web |
| DS21 | The checkbox art is fixed-colour images | priority colours, low being the brand accent (Dark: the accent's dark ink); the shared art in `design/assets/checkbox` takes them as `currentColor`, the check is `checkbox-check` (AWTD-1605) | iOS, Mac, Windows and Android still draw the PNGs, in the art's colours (`#328ACC` low, `#F5AF39` medium, `#D5392E` high, `#B3B6BA` none), so under a partner accent their square stays Astrid blue. Web draws the checkbox in the priority tokens (AWTD-1581), so low priority is the darker accent and follows a partner's brand | closed on web; closed on Android (`CheckboxArt` draws core's vendored `checkbox_{box,repeat,check}.xml`, tinted with the priority token, ADEV-196 `dd8229b`); open on iOS, Mac and Windows until they draw the shared SVG (`generated/apple/AstridDesign.xcassets`, `android/drawable`, `windows/AstridCheckbox.xaml`) |
| DS22 | Priority glyphs as text (`!!` in a header or menu) | the priority colour | Medium's amber is 1.7:1 on white; the web used a darker orange (2.8:1) before adopting the token. "None" as text uses muted text on web. The generator holds each priority colour to 3:1 on the card and lists the amber on a light card (1.67:1 in Light and Ocean) as the one that fails; "none" passes on Dark's card at 3.04:1 | open: text variants of the priority colours, checked for contrast |
| DS23 | Trade-off grid and Decision cards: the five rating fills and their text colours, cells 84-150 wide with a 3 gap and radius 4, a 10 swatch in the key, cards at radius 10 with a 24 number badge | iOS (`DecisionViews.swift`) | iOS draws the dark text as black at 87%, the token is `#111827`. Web reads the gap, cell widths and radius, swatch, card radius and badge from `markdown` (AWTD-1617) | closed on web; open on iOS (the dark text) |
| DS24 | Markdown headings | `type.<density>.markdown-h1` to `-h4`, all semibold (600): comfortable 28 / 22 / 17 / 17, the Dynamic Type steps title2, title3, headline, headline; compact 17 / 15 / 13 / 13, the Mac's `MacMarkdownText`. A heading stops shrinking at h3 and is never smaller than the density's body text, which the generator's tests hold (AWTD-1625) | iOS draws headings one step smaller than the tokens and in its own weights: `MarkdownText.headingFont` is title3 bold, headline, then subheadline, where the tokens say title2, title3, headline, all semibold. The Mac writes its sizes out in `MacMarkdownText.headingSize`. Web reads the comfortable tokens (`app/globals.css`, AWTD-1625) | closed on web; open on iOS (one step small) and Mac (written out) |
| DS25 | A meaning as text or an icon: `accent-ink`, `error-ink`, `success-ink`, `warning-ink`, `info-ink` | Web's (AWTD-1575): it wrote a darker shade for text on light and a lighter one on dark (`text-blue-600 dark:text-blue-400`), and reading the single mid-tone as text dropped links from 5.17:1 to 3.68:1 and error text from 4.83:1 to 3.76:1. Light and Ocean `#1D4ED8`, `#B91C1C`, `#166534`, `#92400E`, `#0E7490` (the success and warning inks were `#15803D` and `#B45309` until DS33: a pill's 15% wash took them to 4.40:1 and 4.47:1); Dark `#60A5FA`, `#F87171`, `#4ADE80`, `#FBBF24`, `#22D3EE`. The error ink is one step darker than web's `#DC2626`, which is 4.14:1 on its own wash; the accent ink was `#2563EB` until DS26 made that the fill, where it is 4.37:1 on its own wash | iOS, Mac, Windows and Android draw text and icons in the fill colour. Web adopts with its next vendored copy. A partner build has to supply `accent-ink` and `accent-ink-dark` with its accent | open on Windows: adopt at the next re-vendor. Adopted on Android (re-vendored `AstridTokens.kt`: `accentInk` `#1D4ED8` / `#60A5FA`, text and icons read `*Ink`, AATD-97 `InkTokensTest`) and on iOS and Mac (AITD-615, core `98596ed`): `Brand.accentInkColor` / `accentInkDarkColor`, error text and links read the inks |
| DS26 | A fill that carries a label: `accent`, `accent-hover`, `error` | Darkened (Jon, 2026-11-11: "Darken accent and error in the shared tokens, every platform follows"; web AWTD-1575) so the label on a button is readable. `accent` `#2563EB` (white on it 5.17:1, was `#3B82F6` at 3.68:1), `accent-hover` `#1D4ED8` (6.70:1, one step on so it is still a hover), `error` `#DC2626` (4.83:1, was `#EF4444` at 3.76:1), in every theme; the generator refuses a label under 4.5:1. What follows: a light theme's `priority-low` and `border-focus` are the accent; `accent-ink` on light themes moves to `#1D4ED8` (DS25); Dark's `border-focus`, `bg-selected-border` and `priority-low` become the accent's dark ink `#60A5FA`, because the new accent is 2.84:1 as a line on a dark card. `priority-low` joined them after web's browser pass found the repeat frame dim: the ink is 5.77:1 on Dark's card and 4.07:1 on a selected row, where the accent was 2.84:1 and 2.00:1; the other priority colours are the same in every theme. What stays: `priority-high` and `overdue` (`#EF4444`: priority's and the due date's own reds, drawn as marks and text, and darker would lose them on dark), the list palette's blue and red (a person's choice), `bg-selected` and light's `bg-selected-border` (tints) | Every client until it re-vendors. iOS and Mac also change `Brand.accentColor`, `Brand.accentHoverColor` and `Brand.accentInkColor`, which the generated Swift names and does not hold; a partner build supplies its own. A client that draws a border, ring or selection outline from `accent` on a dark theme should check it there | open on Windows: adopt at the next re-vendor. Adopted on Android (re-vendored `AstridTokens.kt`: `accent` `#2563EB`, AATD-97) and on iOS and Mac (AITD-615): `Brand.accentColor` `#2563EB`, hover `#1D4ED8`, Dark's lines on the dark ink. Closed on web, which vendors these values |
| DS27 | Categorical colours: a settings tile, a filter's icon, a webhook's kind | Eight hue pairs, `category-<hue>` and `category-<hue>-ink`, from the Tailwind shades web drew (500 for the fill; 700, or 600 for purple, as the light ink; 400 as the dark ink), each ink at 4.5:1 or better (web AWTD-1612) | iOS and Mac draw SwiftUI's system colours (`.orange`, `.purple`, `.pink`), which adapt to the appearance but are not checked for contrast. Web draws the `category-*` pairs (AWTD-1612) | closed on web; open on iOS, Mac (astrid-ios ADEV-165) and Windows; closed on Android (`categoryPurpleInk` replaces the system purple, ADEV-196 `7824acb`) |
| DS28 | An agent's name or icon | `agent` `#A855F7` and `agent-ink` (`#9333EA` light, `#C084FC` dark), readable on the card and on `agent-bubble` (web AWTD-1612) | iOS draws `.purple` (`ChatMessageBubble`, `ListMembershipTab`, the agent picker). Web drew `text-purple-500/600/400` and now reads the tokens. The Apple apps draw the agent bubble as three different purples (`ChatMessageBubble`, `MacChatBubbleStyle`, `MacChatPanelView`) | closed on web; closed on Android (`agentInk` / `agentBubble` replace the system purple, ADEV-196 `7824acb`); open on iOS, Mac (astrid-ios ADEV-165) and Windows |
| DS29 | A surface that is dark in every theme (media viewer, code block, landing footer) | `surface-inverse` `#111827`, the same in every theme, with the dark theme's text on it (web AWTD-1612) | Web wrote `bg-gray-900` and scoped the viewer to `.dark` | open on the other clients where they draw one |
| DS30 | Opening a task on a phone (a one-pane view) | A list row and a board card open task details the same way: the platform's ordinary full-screen push (a slide on iOS, the navigation transition on Android, a full-screen view on mobile web), with no zoom out of the row and no card expanding in place. On iOS, Android and mobile web alike (Jon, 2026-11-14, from astrid-ios AITD-664: "standard on all mobile one column views and part of Astrid-core"). Where the list and the details share the screen, DS7 applies instead | Android conforms (a phone pushes task details full screen, no zoom out of the row). iOS conforms since AITD-664, which removed the AITD-609 zoom from rows and the AITD-642 inline expansion from board cards. Web conforms since AWTD-1653: on a one-column board a card opens the same full-screen view as a list row (`project-status-board.tsx`, `onOpenTask`) | closed on Android, web and iOS |
| DS31 | The square mark that is not yours — somebody else's task, and nobody's | A **2px border in the priority colour**, with the photo or the unassigned glyph inside it and the glyph in that same colour. **No fill.** Every priority is a colour, `priority-none` included: an absent priority is grey, not nothing. Not a new decision — it is what web has always drawn and what DS6 already assumes when it calls the repeat arrows “the photo's border, *in place of the priority outline*” — which is exactly why it went unwritten and a client could diverge without anything noticing (task `e4e47bd7`, split from Windows `f1a8ff9f`) | **Nobody differs, verified 2026-11-14.** Web: `task-leading-control.tsx`, `SQUARE_CLASS` is `border-2` with `borderColor` and the glyph at the same colour, no background class on either square; `getPriorityColor(0)` is `rgb(107,114,128)`. iOS: `TaskRowView.swift` strokes a `RoundedRectangle` with `priorityColor` at `lineWidth: 2` and draws the glyph in it, with `.none → Theme.priorityNone`. Windows adopted it in `f1a8ff9f` and gates it by rendered pixel. Android, verified 2026-11-27 (ADEV-196): `TaskRowView.kt` `LeadingControl` draws the unassigned square as `border(BorderWidth.thick /* 2dp */, priority, square)` with the glyph in `priority` and no background, and someone else's photo through `Avatar(border = priority)`; `colors.priority[0]` is `tokens.priorityNone`. **Initials fallback differs** (not this row's rule, which covers the photo and the glyph): Android puts initials on `colors.accent`, web on `bg-theme-active` with `text-theme-secondary` | closed on web, iOS, Windows and Android. **One exception, by rule** (ADEV-147, core `0f82052`): a *completed* task's leading control is the checkbox whoever holds it, so the square appears only on open tasks |
| DS32 | Opening a task from a board card where the board and the details share the screen | The card opens **the DS7 panel a list row opens**, never an editor expanded inside the column (Jon, 2026-11-15, astrid-ios AITD-679: "Stop opening board task details by expanding the board task. Open, just like task details from a task row, but pick the left or right side based on the spacing on screen"). Its side, its clamped box and its arrow are `rows::callout::place` (`Rule::CalloutPlacement`, fixture `contracts/fixtures/callout-placement.json`): the side with more room beside the card, ties right, decided once at the tap so the panel does not hop sides as the board scrolls. **The panel touches the card**: its arrow's tip sits on the card's edge, never docked to the window's edge and never a gutter away (Jon, 2026-11-15, on the iPad build: "the arrow doesn't touch the task row… It should. Update everywhere to match this"; core PR #59). The caller measures the panel with its arrow's reach included. Everything else is DS7's: the selection outline on the panel and on the card, the `motion.panel` reveal, the arrow filled with the details surface and overlapping the panel by a point. Docked on the leading side the arrow moves to the panel's trailing edge, mirrored to point at the card, and the reveal grows out of that edge. A tapped card is not scrolled under its own panel. The caller measures the panel; the rule only places it. A one-pane board pushes instead (DS30) | iOS (iPad board) and the Mac's two boards conform since AITD-679, at core `b0947e8`. Web conforms: a card opens `BoardTaskCallout` beside it at every width, its arrow on the card's edge (astrid-web `0db8147e`, `8d2e2cf9`). Windows spliced the detail into the column as another card (task `011080d4` landed the core rule; the board still has to adopt it). Android tablets have no board details yet | closed on web; open on Windows (ADEV-25); Android adopts when it builds them |
| DS33 | A pill: board status, completed-on, counts, Done-since | One capsule (`pill`, ADEV-153). **The wash is 15%** (Jon, 2026-11-20; iOS is the reference, not the general 12% `INK_WASH`), the stroke the same colour at 40%, 1 wide, padding 8 across and 2 down, a capsule radius, `type.comfortable.caption1`. **The label is the ink** (`<meaning>-ink`), not the fill colour iOS draws: DS25 says text on a surface is the ink, and the generator holds each pill ink to 4.5:1 on its own 15% wash over the card in every theme, which took Light and Ocean `success-ink` to `#166534` and `warning-ink` to `#92400E`. `pill.states` is the board state to meaning map: inbox `error`, waiting `warning`, ready `success`, doing `info`, done `accent`. Counts and the Done-since pill draw the neutral grey wash the web pastes in four files; that stays the client halves' to settle | iOS draws the fill colour as text (`TaskBoardStatusPill.swift`, `CompletedOnPill.swift`) and restates the capsule. Web has `.board-status-pill` (`styles/components.css`) and a grey class string in four files | open on iOS and web: adopt at the next re-vendor (separate tasks on each board) |
| DS34 | Board column: width, gap, header, wall, body | `layout.<density>.board-column-*` (ADEV-154, from iOS and the Mac). **Comfortable:** columns share the width, none narrower than `board-column-min-width` 300 and at most five abreast (`boardColumnsVisible`); 16 between columns; the header 12 across, 12 above, 8 below; a 6 `board-column-wall` of the frame colour each side of a recessed well; the well 6 across and 8 down; cards 6 apart. **Compact:** the board inset 16 (`board-padding`), columns at least 250 and sharing the rest, 12 apart, one 10 padding around header and body, 8 from header to cards, cards 8 apart. The header is the column's name in `headline` with its count (DS37) on the trailing edge, and a two-line muted description under it except on Done | Web: one shared column (`components/board-column.tsx`) at `min-w-72 max-w-md` (288, capped at 448) with `gap-3` (12) between columns on touch too; the header is `px-3 py-2` (12 / 8) with a bottom border, the body `p-2` (8); one bordered surface with no wall or well. iOS and the Mac draw the token values but type them out (`BoardColumnView.swift`, `ProjectStatus.swift` `targetMinColumnWidth: 300`, `MacLayout.swift`) | open on web; iOS and Mac read the tokens at their next re-vendor |
| DS35 | Board card | Radius per DS13. Padding `board-card-padding-x` × `-y`: 10 × 12 comfortable, 8 × 8 compact, tighter than a list row because the column's well already insets it. The card surface, a `size.hairline` stroke in `border`, and when selected the DS5 outline. The card draws the only chrome: the row inside it adds no background or border of its own | Web: `.board-card` is also a `.task-row`, so it takes the row's padding (16 × 14 / 12 × 9, DS14), not the card's. iOS (`BoardTaskCardView.swift`, `TaskRowView.swift` `embeddedInCard`) and the Mac (`MacBoardView.swift`) draw the token numbers but type them out | open on web; iOS and Mac read the tokens at their next re-vendor |
| DS36 | An empty board column | The one empty state, shared by iOS and the Mac since astrid-ios ADEV-166: the art beside a speech bubble whose tail points at it. The art is `layout.<density>.empty-art`, 160 comfortable and 96 compact, and none on a project board's short rows. The bubble pads 16 (12 compact), radius 16, a 2 border. The message is the column's own (`BoardColumnEmptyMessage`). A comfortable column holds at least `empty-column-min-height` 260 while empty | Web: `AstridEmptyState` sizes the art at a third of its width (`w-1/3`), not a fixed size. The Mac conforms since ADEV-166 (96; it was 120) | open on web |
| DS37 | A column header's count and the Done-since control | One header badge, iOS's `BoardHeaderBadge` (Jon, 2026-11-20: the Done header is iOS's capsule, “Done · 15 ✓ This week” with a chevron, and the capsule itself is the button). `caption1`, `text-secondary`, a `bg-tertiary` capsule with no stroke, 8 across and 4 down. The count and the Done-since trigger wear the same badge, on the header's trailing edge | Web draws both as the neutral `Pill` (DS33): the muted colour at 15% with a 40% stroke, 2 down. The Mac conforms since ADEV-166 (it was plain caption text) | open on web: settle against DS33's neutral pill (one of the two changes) |
