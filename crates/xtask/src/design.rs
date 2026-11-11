//! The design tokens: one source, generated into each client's own format.
//!
//! `design/tokens.json` is to how Astrid LOOKS what `contracts/fixtures` is to how it behaves: one
//! file every client reads the same values from, instead of five palettes kept alike by comments
//! saying "matches iOS". Before it, Android copied `Theme.swift`, Windows copied the web's
//! stylesheets, the web had three different yellows for "medium priority", and nothing failed when
//! any of them moved.
//!
//! ## Generated at build time, never shipped at run time
//!
//! `astrid_rules::theme` says the core does not hand a shell hex values to paste into brushes, and
//! that still holds: nothing here is linked into the core. `cargo xtask gen-design` writes native
//! resources — CSS custom properties, a Swift file — that a client vendors and its own toolkit
//! themes with. The values are shared; the mechanism stays the platform's.
//!
//! ## A theme is data
//!
//! A theme is an entry under `themes`, naming its appearance and, usually, the theme it `extends`.
//! Every theme must resolve the same set of colour names, so a client can never ask a theme for a
//! token it does not have, and must keep its body text readable on its own page (the web's
//! AWTD-1496, enforced at the source so a fifth theme cannot reintroduce it).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// Where the source lives, relative to the repository root.
pub const SOURCE: &str = "design/tokens.json";

/// Where the generated files are written, relative to the repository root.
pub const GENERATED: &str = "design/generated";

/// What a consumer vendors, and where: (file under `GENERATED`, path in the consumer).
const WEB_VENDORED: &[(&str, &str)] = &[
    (
        "web/astrid-tokens.css",
        "packages/astrid-design/astrid-tokens.css",
    ),
    (
        "tokens.resolved.json",
        "packages/astrid-design/tokens.resolved.json",
    ),
];

/// The revision file beside a consumer's vendored copies.
const WEB_REVISION: &str = "packages/astrid-design/REVISION";

/// WCAG AA for body text.
const AA_BODY: f64 = 4.5;

/// The text colours that must stay readable, and the surfaces they are drawn straight onto.
const READABLE_TEXT: &[&str] = &["text-primary", "text-secondary", "text-muted"];
const READABLE_ON: &[&str] = &["bg-primary", "bg-secondary"];

/// A meaning drawn as text or an icon: the `<meaning>-ink` colours. The base colour (`accent`,
/// `error`, …) is one mid-tone that is right as a fill and too light as text on a light surface;
/// the ink is the same meaning dark enough (or, on a dark theme, light enough) to read.
const INK_MEANINGS: &[&str] = &["accent", "error", "success", "warning", "info"];

/// How strong a status wash is: the meaning's base colour at this opacity over the surface, behind
/// a notice or a badge whose label is the ink. The web's `--theme-wash-opacity`.
const INK_WASH: f64 = 0.12;

/// The surfaces an ink is drawn on: `bg-secondary`, and the card. The tokens do not name the card
/// yet (register DS9); it is the web's `--theme-surface`: `bg-hover` (the white) on a light theme,
/// `bg-secondary` on a dark one. The page (`bg-primary`) is not held to this, because a theme's
/// page may be a colour (Ocean's cyan), where a status colour is not drawn straight onto it.
fn ink_surfaces(appearance: &str) -> &'static [&'static str] {
    if appearance == "dark" {
        &["bg-secondary"]
    } else {
        &["bg-hover", "bg-secondary"]
    }
}

fn ink_card(appearance: &str) -> &'static str {
    ink_surfaces(appearance)[0]
}

/// A solid fill and the text a client draws on it: (fill, label). A button's label is body text, so
/// it is held to the same 4.5:1. No token names the text on an error fill; every client that
/// draws one (the web's `--destructive-foreground`, `text-danger-foreground`) uses `accent-text`.
/// `success`, `warning` and `info` are not held to this: they are washes and marks, and white on
/// the warning amber is 2.1:1 (the web's spec says to prefer a wash there).
const LABELLED_FILLS: &[(&str, &str)] = &[
    ("accent", "accent-text"),
    ("accent-hover", "accent-text"),
    ("error", "accent-text"),
];

/// WCAG 1.4.11 for a line that carries meaning on its own: the focus ring.
const AA_LINE: f64 = 3.0;

/// A line that must be seen against the page it is drawn on.
const VISIBLE_LINES: &[&str] = &["border-focus"];

/// One colour: a literal, or the brand's, which a partner build replaces.
#[derive(Debug, Clone, PartialEq)]
pub enum Colour {
    Hex(u8, u8, u8),
    Brand(String),
}

impl Colour {
    fn parse(text: &str) -> Option<Colour> {
        if let Some(name) = text
            .strip_prefix("{brand.")
            .and_then(|t| t.strip_suffix('}'))
        {
            return Some(Colour::Brand(name.to_string()));
        }
        let hex = text.strip_prefix('#')?;
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
        Some(Colour::Hex(byte(0)?, byte(2)?, byte(4)?))
    }

    fn source(&self) -> String {
        match self {
            Colour::Hex(r, g, b) => format!("#{r:02X}{g:02X}{b:02X}"),
            Colour::Brand(name) => format!("{{brand.{name}}}"),
        }
    }
}

/// A theme with its `extends` chain flattened.
#[derive(Debug, Clone)]
pub struct Theme {
    pub appearance: String,
    pub colours: BTreeMap<String, Colour>,
}

/// The tokens, checked and flattened.
#[derive(Debug, Clone)]
pub struct Tokens {
    pub brand: BTreeMap<String, (u8, u8, u8)>,
    /// In the order the source lists them.
    pub themes: Vec<(String, Theme)>,
    /// The source as read, for the sections that pass through unchanged.
    pub raw: Map<String, Value>,
}

/// Read and check the source text. Every problem is reported, not just the first.
pub fn parse(source: &str, theme_names: &[&str]) -> Result<Tokens, Vec<String>> {
    let value: Value = serde_json::from_str(source).map_err(|e| vec![format!("not JSON: {e}")])?;
    let Some(raw) = value.as_object().cloned() else {
        return Err(vec!["the top level must be an object".to_string()]);
    };
    let mut problems = Vec::new();

    if raw.get("version").and_then(Value::as_u64) != Some(1) {
        problems.push("version must be 1".to_string());
    }

    let mut brand = BTreeMap::new();
    for (name, value) in object(&raw, "brand") {
        match value.as_str().and_then(Colour::parse) {
            Some(Colour::Hex(r, g, b)) => {
                brand.insert(name.clone(), (r, g, b));
            }
            _ => problems.push(format!("brand.{name} must be a #RRGGBB literal")),
        }
    }

    let declared = object(&raw, "themes");
    let mut themes = Vec::new();
    for name in theme_order(source, &declared) {
        match flatten(&name, &declared, &brand) {
            Ok(theme) => themes.push((name, theme)),
            Err(mut errors) => problems.append(&mut errors),
        }
    }

    // Every theme answers for the same names, so no client asks a theme for a token it lacks.
    if let Some((first_name, first)) = themes.first() {
        for (name, theme) in &themes[1..] {
            for key in theme.colours.keys() {
                if !first.colours.contains_key(key) {
                    problems.push(format!(
                        "themes.{name}.colors.{key} is not defined in {first_name}; add it there \
                         so every theme has it"
                    ));
                }
            }
        }
    }

    for (name, theme) in &themes {
        for text in READABLE_TEXT {
            for surface in READABLE_ON {
                let (Some(Colour::Hex(tr, tg, tb)), Some(Colour::Hex(sr, sg, sb))) =
                    (theme.colours.get(*text), theme.colours.get(*surface))
                else {
                    continue;
                };
                let ratio = contrast((*tr, *tg, *tb), (*sr, *sg, *sb));
                if ratio < AA_BODY {
                    problems.push(format!(
                        "themes.{name}: {text} on {surface} is {ratio:.2}:1, under {AA_BODY}:1"
                    ));
                }
            }
        }
    }

    // A meaning as text reads on the theme's surfaces, and on its own wash over the card. A brand
    // colour is checked at Astrid's value; a partner's is the partner's to check.
    let literal = |theme: &Theme, key: &str| match theme.colours.get(key)? {
        Colour::Hex(r, g, b) => Some((*r, *g, *b)),
        Colour::Brand(reference) => brand.get(reference).copied(),
    };
    for (name, theme) in &themes {
        for meaning in INK_MEANINGS {
            let ink_name = format!("{meaning}-ink");
            let (Some(ink), Some(base)) = (literal(theme, &ink_name), literal(theme, meaning))
            else {
                continue;
            };
            for surface in ink_surfaces(&theme.appearance) {
                let Some(on) = literal(theme, surface) else {
                    continue;
                };
                let ratio = contrast(ink, on);
                if ratio < AA_BODY {
                    problems.push(format!(
                        "themes.{name}: {ink_name} on {surface} is {ratio:.2}:1, under {AA_BODY}:1"
                    ));
                }
            }
            let card = ink_card(&theme.appearance);
            if let Some(on) = literal(theme, card) {
                let ratio = contrast(ink, over(base, on, INK_WASH));
                if ratio < AA_BODY {
                    problems.push(format!(
                        "themes.{name}: {ink_name} on the {meaning} wash over {card} is \
                         {ratio:.2}:1, under {AA_BODY}:1"
                    ));
                }
            }
        }
    }

    // A label on a solid fill reads (astrid-web AWTD-1575: white on the old accent was 3.68:1 and
    // on the old error red 3.76:1), and the focus ring shows on the theme's own surfaces: a fill
    // dark enough for a white label is too dark to be a line on a dark theme.
    for (name, theme) in &themes {
        for (fill, label) in LABELLED_FILLS {
            let (Some(on), Some(text)) = (literal(theme, fill), literal(theme, label)) else {
                continue;
            };
            let ratio = contrast(text, on);
            if ratio < AA_BODY {
                problems.push(format!(
                    "themes.{name}: {label} on {fill} is {ratio:.2}:1, under {AA_BODY}:1"
                ));
            }
        }
        for line in VISIBLE_LINES {
            for surface in READABLE_ON {
                let (Some(colour), Some(on)) = (literal(theme, line), literal(theme, surface))
                else {
                    continue;
                };
                let ratio = contrast(colour, on);
                if ratio < AA_LINE {
                    problems.push(format!(
                        "themes.{name}: {line} on {surface} is {ratio:.2}:1, under {AA_LINE}:1"
                    ));
                }
            }
        }
    }

    // The theme list and the core's theme modes are one list. A theme here that the core cannot
    // name is one nobody can choose; a mode there with no colours is one that cannot be drawn.
    let mut have: Vec<&str> = themes.iter().map(|(name, _)| name.as_str()).collect();
    let mut want: Vec<&str> = theme_names.to_vec();
    have.sort_unstable();
    want.sort_unstable();
    if have != want {
        problems.push(format!(
            "themes are {have:?} but astrid_rules::theme names {want:?}; add the theme to both"
        ));
    }

    for (name, list) in object(&raw, "palette") {
        for (index, entry) in list.as_array().into_iter().flatten().enumerate() {
            if !matches!(
                entry.as_str().and_then(Colour::parse),
                Some(Colour::Hex(..))
            ) {
                problems.push(format!("palette.{name}[{index}] must be a #RRGGBB literal"));
            }
        }
    }

    let space: Vec<f64> = raw
        .get("space")
        .and_then(Value::as_array)
        .map(|steps| steps.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    if space.is_empty() || space.windows(2).any(|pair| pair[0] >= pair[1]) {
        problems.push("space must be an ascending list of numbers".to_string());
    }

    if problems.is_empty() {
        Ok(Tokens { brand, themes, raw })
    } else {
        Err(problems)
    }
}

fn object(raw: &Map<String, Value>, key: &str) -> Map<String, Value> {
    raw.get(key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The themes in the order the file lists them, which a JSON map does not keep.
fn theme_order(source: &str, declared: &Map<String, Value>) -> Vec<String> {
    let mut names: Vec<(usize, String)> = declared
        .keys()
        .map(|name| {
            (
                source
                    .find(&format!("\"{name}\": {{"))
                    .unwrap_or(usize::MAX),
                name.clone(),
            )
        })
        .collect();
    names.sort();
    names.into_iter().map(|(_, name)| name).collect()
}

fn flatten(
    name: &str,
    declared: &Map<String, Value>,
    brand: &BTreeMap<String, (u8, u8, u8)>,
) -> Result<Theme, Vec<String>> {
    let mut problems = Vec::new();
    let mut chain = vec![name.to_string()];
    loop {
        let current = chain.last().expect("the chain starts with one name");
        let Some(theme) = declared.get(current).and_then(Value::as_object) else {
            return Err(vec![format!(
                "themes.{name} extends {current}, which is not a theme"
            )]);
        };
        match theme.get("extends").and_then(Value::as_str) {
            Some(parent) if chain.iter().any(|seen| seen == parent) => {
                return Err(vec![format!(
                    "themes.{name} extends itself through {parent}"
                )]);
            }
            Some(parent) => chain.push(parent.to_string()),
            None => break,
        }
    }

    let own = declared[name]
        .as_object()
        .expect("checked in the loop above");
    let appearance = own
        .get("appearance")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if appearance != "light" && appearance != "dark" {
        problems.push(format!(
            "themes.{name}.appearance must be \"light\" or \"dark\""
        ));
    }

    let mut colours = BTreeMap::new();
    for ancestor in chain.iter().rev() {
        let layer = declared[ancestor]
            .as_object()
            .expect("checked in the loop above");
        for (key, value) in object(layer, "colors") {
            if !is_kebab(&key) {
                problems.push(format!("themes.{ancestor}.colors.{key} must be kebab-case"));
            }
            match value.as_str().and_then(Colour::parse) {
                Some(Colour::Brand(reference)) if !brand.contains_key(&reference) => problems
                    .push(format!("themes.{ancestor}.colors.{key} names brand.{reference}, which does not exist")),
                Some(colour) => {
                    colours.insert(key, colour);
                }
                None => problems.push(format!(
                    "themes.{ancestor}.colors.{key} must be #RRGGBB or {{brand.<name>}}"
                )),
            }
        }
    }

    if problems.is_empty() {
        Ok(Theme {
            appearance,
            colours,
        })
    } else {
        Err(problems)
    }
}

fn is_kebab(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !key.starts_with('-')
        && !key.ends_with('-')
}

/// WCAG 2.x contrast ratio.
pub fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
    fn luminance((r, g, b): (u8, u8, u8)) -> f64 {
        let linear = |channel: u8| {
            let s = f64::from(channel) / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
    }
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `top` at `alpha` over `under`, as a browser composites it: per channel, rounded.
pub fn over(top: (u8, u8, u8), under: (u8, u8, u8), alpha: f64) -> (u8, u8, u8) {
    let mix = |t: u8, u: u8| (f64::from(t) * alpha + f64::from(u) * (1.0 - alpha)).round() as u8;
    (
        mix(top.0, under.0),
        mix(top.1, under.1),
        mix(top.2, under.2),
    )
}

// ── Emitters ──────────────────────────────────────────────────────────────────────────────────

/// Every generated file: (path under `GENERATED`, contents).
pub fn outputs(tokens: &Tokens) -> Vec<(&'static str, String)> {
    vec![
        ("tokens.resolved.json", resolved_json(tokens)),
        (
            "web/astrid-tokens.css",
            stamped("/*", " */", &web_css(tokens)),
        ),
        (
            "apple/AstridTokens.swift",
            stamped("//", "", &apple_swift(tokens)),
        ),
    ]
}

/// The header every generated source file carries: where it came from, and a hash of everything
/// below it, so a consumer can tell a vendored copy from a hand-edited one without this repository.
fn stamped(open: &str, close: &str, body: &str) -> String {
    format!(
        "{open} GENERATED by `cargo xtask gen-design` from astrid-core design/tokens.json. Do not edit.{close}\n\
         {open} body-sha256: {}{close}\n{body}",
        sha256(body)
    )
}

pub fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The source with every theme's `extends` flattened: what a client's own emitter or test reads.
fn resolved_json(tokens: &Tokens) -> String {
    let mut out = tokens.raw.clone();
    let mut themes = Map::new();
    for (name, theme) in &tokens.themes {
        let colours: Map<String, Value> = theme
            .colours
            .iter()
            .map(|(key, colour)| (key.clone(), Value::String(colour.source())))
            .collect();
        let mut entry = Map::new();
        entry.insert(
            "appearance".to_string(),
            Value::String(theme.appearance.clone()),
        );
        entry.insert("colors".to_string(), Value::Object(colours));
        themes.insert(name.clone(), Value::Object(entry));
    }
    out.insert("themes".to_string(), Value::Object(themes));
    out.insert(
        "themeOrder".to_string(),
        Value::Array(
            tokens
                .themes
                .iter()
                .map(|(name, _)| Value::String(name.clone()))
                .collect(),
        ),
    );
    let mut text = serde_json::to_string_pretty(&Value::Object(out)).expect("a JSON value prints");
    text.push('\n');
    text
}

fn number(value: &Value) -> String {
    match value.as_f64() {
        Some(n) if n.fract() == 0.0 => format!("{n:.0}"),
        Some(n) => format!("{n}"),
        None => value.as_str().unwrap_or_default().to_string(),
    }
}

/// CSS custom properties. Colours are `r, g, b` triplets, because the web composes them as
/// `rgb(var(--x))` and `rgba(var(--x), 0.5)`; a brand colour falls back to Astrid's own.
fn web_css(tokens: &Tokens) -> String {
    let mut css = String::from(":root {\n");
    for (name, (r, g, b)) in &tokens.brand {
        css.push_str(&format!("  --astrid-brand-{name}: {r}, {g}, {b};\n"));
    }
    for (name, list) in object(&tokens.raw, "palette") {
        for (index, entry) in list.as_array().into_iter().flatten().enumerate() {
            if let Some(Colour::Hex(r, g, b)) = entry.as_str().and_then(Colour::parse) {
                css.push_str(&format!(
                    "  --astrid-palette-{name}-{}: {r}, {g}, {b};\n",
                    index + 1
                ));
            }
        }
    }
    for step in tokens
        .raw
        .get("space")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        css.push_str(&format!("  --astrid-space-{0}: {0}px;\n", number(step)));
    }
    for (name, value) in object(&tokens.raw, "radius") {
        css.push_str(&format!(
            "  --astrid-radius-{name}: {}px;\n",
            number(&value)
        ));
    }
    for (name, shadow) in object(&tokens.raw, "shadow") {
        css.push_str(&format!(
            "  --astrid-shadow-{name}: 0 {}px {}px rgba(0, 0, 0, {});\n",
            number(&shadow["y"]),
            number(&shadow["blur"]),
            number(&shadow["alpha"])
        ));
    }
    for (density, styles) in object(&tokens.raw, "type") {
        for (name, style) in styles.as_object().into_iter().flatten() {
            css.push_str(&format!(
                "  --astrid-type-{density}-{name}-size: {}px;\n  --astrid-type-{density}-{name}-weight: {};\n",
                number(&style["size"]),
                number(&style["weight"])
            ));
        }
    }
    for (name, motion) in object(&tokens.raw, "motion") {
        css.push_str(&format!(
            "  --astrid-motion-{name}-duration: {}ms;\n",
            number(&motion["ms"])
        ));
        if let Some(ease) = motion.get("ease").and_then(Value::as_str) {
            css.push_str(&format!("  --astrid-motion-{name}-ease: {ease};\n"));
        }
        if let Some(damping) = motion.get("damping") {
            css.push_str(&format!(
                "  --astrid-motion-{name}-damping: {};\n",
                number(damping)
            ));
        }
    }
    // `size`, `layout` and `markdown` share a shape: lengths, per density or holding everywhere.
    // A length is in px; a ratio (`*-scale`) has no unit.
    for section in ["size", "layout", "markdown"] {
        for (name, value) in object(&tokens.raw, section) {
            match value.as_object() {
                Some(sizes) => {
                    for (key, size) in sizes {
                        css.push_str(&format!(
                            "  --astrid-{section}-{name}-{key}: {}{};\n",
                            number(size),
                            unit(key)
                        ));
                    }
                }
                None => css.push_str(&format!(
                    "  --astrid-{section}-{name}: {}{};\n",
                    number(&value),
                    unit(&name)
                )),
            }
        }
    }
    for (name, value) in object(&tokens.raw, "state") {
        css.push_str(&format!("  --astrid-state-{name}: {};\n", number(&value)));
    }
    css.push_str("}\n");

    for (name, theme) in &tokens.themes {
        css.push_str(&format!(
            "\n.{name} {{\n  color-scheme: {};\n",
            theme.appearance
        ));
        for (key, colour) in &theme.colours {
            let value = match colour {
                Colour::Hex(r, g, b) => format!("{r}, {g}, {b}"),
                Colour::Brand(reference) => {
                    let (r, g, b) = tokens.brand[reference];
                    format!("var(--brand-{reference}-rgb, {r}, {g}, {b})")
                }
            };
            css.push_str(&format!("  --astrid-{key}: {value};\n"));
        }
        css.push_str("}\n");
    }
    css
}

/// The CSS unit a length token takes. A ratio is a bare number.
fn unit(key: &str) -> &'static str {
    if key.ends_with("-scale") {
        ""
    } else {
        "px"
    }
}

fn camel(key: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for character in key.chars() {
        if character == '-' {
            upper = true;
        } else if upper {
            out.extend(character.to_uppercase());
            upper = false;
        } else {
            out.push(character);
        }
    }
    out
}

/// SwiftUI. Brand colours stay `Brand.*`, which the Apple apps already read from Info.plist.
fn apple_swift(tokens: &Tokens) -> String {
    let brand_name = |reference: &str| match reference {
        "accent" => "Brand.accentColor".to_string(),
        "accent-hover" => "Brand.accentHoverColor".to_string(),
        "accent-text" => "Brand.accentTextColor".to_string(),
        "accent-ink" => "Brand.accentInkColor".to_string(),
        "accent-ink-dark" => "Brand.accentInkDarkColor".to_string(),
        other => format!("Brand.{}Color", camel(other)),
    };
    let mut swift = String::from("import SwiftUI\n\nenum AstridTokens {\n");
    swift.push_str("    enum Appearance { case light, dark }\n\n    struct Palette {\n        let appearance: Appearance\n");
    let keys: Vec<&String> = tokens
        .themes
        .first()
        .map(|(_, theme)| theme.colours.keys().collect())
        .unwrap_or_default();
    for key in &keys {
        swift.push_str(&format!("        let {}: Color\n", camel(key)));
    }
    swift.push_str("    }\n");
    for (name, theme) in &tokens.themes {
        swift.push_str(&format!(
            "\n    static let {} = Palette(\n        appearance: .{}",
            camel(name),
            theme.appearance
        ));
        for key in &keys {
            let value = match &theme.colours[*key] {
                Colour::Hex(r, g, b) => {
                    format!("Color(red: {r}/255, green: {g}/255, blue: {b}/255)")
                }
                Colour::Brand(reference) => brand_name(reference),
            };
            swift.push_str(&format!(",\n        {}: {value}", camel(key)));
        }
        swift.push_str("\n    )\n");
    }
    swift.push_str("\n    /// Every theme by the name the core's theme mode travels under.\n    static let themes: [String: Palette] = [");
    let entries: Vec<String> = tokens
        .themes
        .iter()
        .map(|(name, _)| format!("\"{name}\": {}", camel(name)))
        .collect();
    swift.push_str(&entries.join(", "));
    swift.push_str("]\n");

    swift.push_str("\n    enum Space {\n");
    for step in tokens
        .raw
        .get("space")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        swift.push_str(&format!(
            "        static let s{0}: CGFloat = {0}\n",
            number(step)
        ));
    }
    swift.push_str("    }\n\n    enum Radius {\n");
    for (name, value) in object(&tokens.raw, "radius") {
        swift.push_str(&format!(
            "        static let {}: CGFloat = {}\n",
            camel(&name),
            number(&value)
        ));
    }
    swift.push_str("    }\n\n    enum Shadow {\n");
    for (name, shadow) in object(&tokens.raw, "shadow") {
        swift.push_str(&format!(
            "        static let {}: (color: Color, radius: CGFloat, x: CGFloat, y: CGFloat) = (Color.black.opacity({}), {}, 0, {})\n",
            camel(&name),
            number(&shadow["alpha"]),
            number(&shadow["blur"]),
            number(&shadow["y"])
        ));
    }
    swift.push_str("    }\n\n    enum TypeSize {\n");
    for (density, styles) in object(&tokens.raw, "type") {
        swift.push_str(&format!("        enum {} {{\n", capitalised(&density)));
        for (name, style) in styles.as_object().into_iter().flatten() {
            swift.push_str(&format!(
                "            static let {}: (size: CGFloat, weight: Int) = ({}, {})\n",
                camel(name),
                number(&style["size"]),
                number(&style["weight"])
            ));
        }
        swift.push_str("        }\n");
    }
    swift.push_str("    }\n\n    enum Motion {\n");
    for (name, motion) in object(&tokens.raw, "motion") {
        swift.push_str(&format!(
            "        static let {}Duration: Double = {}\n",
            camel(&name),
            motion["ms"].as_f64().unwrap_or_default() / 1000.0
        ));
        if let Some(damping) = motion.get("damping") {
            swift.push_str(&format!(
                "        static let {}Damping: Double = {}\n",
                camel(&name),
                number(damping)
            ));
        }
    }
    swift.push_str("    }\n");
    for section in ["size", "layout", "markdown"] {
        swift.push_str(&format!("\n    enum {} {{\n", capitalised(section)));
        for (name, value) in object(&tokens.raw, section) {
            match value.as_object() {
                Some(sizes) => {
                    swift.push_str(&format!("        enum {} {{\n", capitalised(&name)));
                    for (key, size) in sizes {
                        swift.push_str(&format!(
                            "            static let {}: CGFloat = {}\n",
                            camel(key),
                            number(size)
                        ));
                    }
                    swift.push_str("        }\n");
                }
                None => swift.push_str(&format!(
                    "        static let {}: CGFloat = {}\n",
                    camel(&name),
                    number(&value)
                )),
            }
        }
        swift.push_str("    }\n");
    }
    swift.push_str("\n    enum State {\n");
    for (name, value) in object(&tokens.raw, "state") {
        swift.push_str(&format!(
            "        static let {}: Double = {}\n",
            camel(&name),
            number(&value)
        ));
    }
    swift.push_str("    }\n}\n");
    swift
}

fn capitalised(name: &str) -> String {
    let camel = camel(name);
    let mut characters = camel.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => camel,
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────────────────────

/// The theme names the core can store, without `auto`, which is a choice between two of them.
pub fn core_theme_names() -> Vec<&'static str> {
    astrid_rules::theme::Theme::all()
        .into_iter()
        .filter(|theme| theme.is_dark().is_some())
        .map(astrid_rules::theme::Theme::wire)
        .collect()
}

fn load(root: &Path) -> Result<Tokens, ExitCode> {
    let path = root.join(SOURCE);
    let source = std::fs::read_to_string(&path).map_err(|err| {
        eprintln!("cannot read {}: {err}", path.display());
        ExitCode::FAILURE
    })?;
    parse(&source, &core_theme_names()).map_err(|problems| {
        eprintln!("{SOURCE} is not valid:");
        for problem in problems {
            eprintln!("  {problem}");
        }
        ExitCode::FAILURE
    })
}

/// `cargo xtask gen-design`: write every generated file.
pub fn generate(root: &Path) -> ExitCode {
    let tokens = match load(root) {
        Ok(tokens) => tokens,
        Err(code) => return code,
    };
    for (relative, contents) in outputs(&tokens) {
        let path = root.join(GENERATED).join(relative);
        if let Some(parent) = path.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                eprintln!("cannot create {}: {err}", parent.display());
                return ExitCode::FAILURE;
            }
        }
        if let Err(err) = std::fs::write(&path, contents) {
            eprintln!("cannot write {}: {err}", path.display());
            return ExitCode::FAILURE;
        }
        println!("wrote {GENERATED}/{relative}");
    }
    ExitCode::SUCCESS
}

/// `cargo xtask check-design [astrid-web checkout]`: the generated files are what the source
/// produces, and a consumer's vendored copies are this repository's at the revision it names.
pub fn check(root: &Path, web: Option<PathBuf>) -> ExitCode {
    let tokens = match load(root) {
        Ok(tokens) => tokens,
        Err(code) => return code,
    };
    let mut stale = 0;
    for (relative, expected) in outputs(&tokens) {
        let path = root.join(GENERATED).join(relative);
        match std::fs::read_to_string(&path) {
            Ok(actual) if lf(&actual) == lf(&expected) => {
                println!("ok       {GENERATED}/{relative}")
            }
            Ok(_) => {
                println!("STALE    {GENERATED}/{relative}");
                stale += 1;
            }
            Err(_) => {
                println!("MISSING  {GENERATED}/{relative}");
                stale += 1;
            }
        }
    }
    if stale > 0 {
        eprintln!(
            "\n{stale} generated file(s) do not match {SOURCE}. Run `cargo xtask gen-design`."
        );
        return ExitCode::FAILURE;
    }

    // Named on the command line: a difference is real. Found beside this repository: a working
    // copy there is routinely behind its own remote, so a difference is printed and not failed —
    // the same rule `check-pins` follows.
    let named = web.is_some();
    let web = web.unwrap_or_else(|| root.join("../astrid-web"));
    match vendored_differences(root, &web) {
        Vendored::NotAdopted => println!(
            "\nastrid-web: no {WEB_REVISION} at {}; unchecked",
            web.display()
        ),
        Vendored::Unavailable(why) => println!("\nastrid-web: {why}; unchecked"),
        Vendored::Compared(differences) if differences.is_empty() => {
            println!("\nastrid-web: vendored copies match its pinned revision");
        }
        Vendored::Compared(differences) => {
            for difference in &differences {
                println!("\nastrid-web: {difference}");
            }
            if named {
                eprintln!("\nastrid-web's vendored design tokens are not this repository's at its REVISION.");
                return ExitCode::FAILURE;
            }
            println!("(found beside this repository rather than named; it may just need a pull)");
        }
    }
    ExitCode::SUCCESS
}

enum Vendored {
    NotAdopted,
    Unavailable(String),
    Compared(Vec<String>),
}

fn vendored_differences(root: &Path, web: &Path) -> Vendored {
    let Ok(revision) = std::fs::read_to_string(web.join(WEB_REVISION)) else {
        return Vendored::NotAdopted;
    };
    let revision = revision.trim();
    let mut differences = Vec::new();
    for (generated, vendored) in WEB_VENDORED {
        let spec = format!("{revision}:{GENERATED}/{generated}");
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["show", &spec])
            .output();
        let ours = match output {
            Ok(output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).to_string()
            }
            _ => return Vendored::Unavailable(format!("{spec} is not in this checkout's history")),
        };
        match std::fs::read_to_string(web.join(vendored)) {
            Ok(theirs) if lf(&theirs) == lf(&ours) => {}
            Ok(_) => differences.push(format!("{vendored} differs from {spec}")),
            Err(_) => differences.push(format!("{vendored} is missing")),
        }
    }
    Vendored::Compared(differences)
}

fn lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const THEMES: &[&str] = &["light", "dark"];

    fn source(dark_extra: &str, muted: &str) -> String {
        format!(
            r##"{{
  "version": 1,
  "brand": {{ "accent": "#3B82F6" }},
  "themes": {{
    "light": {{ "appearance": "light", "colors": {{
      "bg-primary": "#FFFFFF", "bg-secondary": "#F9FAFB",
      "text-primary": "#111827", "text-secondary": "#4B5563", "text-muted": "{muted}",
      "accent": "{{brand.accent}}" }} }},
    "dark": {{ "appearance": "dark", "extends": "light", "colors": {{
      "bg-primary": "#111827", "bg-secondary": "#1F2937",
      "text-primary": "#FFFFFF", "text-secondary": "#D1D5DB", "text-muted": "#9CA3AF"{dark_extra} }} }}
  }},
  "palette": {{ "list": ["#EF4444"] }},
  "space": [2, 4],
  "radius": {{ "sm": 6 }},
  "shadow": {{ "sm": {{ "y": 1, "blur": 2, "alpha": 0.05 }} }},
  "type": {{ "comfortable": {{ "body": {{ "size": 17, "weight": 400 }} }} }},
  "motion": {{ "fast": {{ "ms": 150, "ease": "ease-out" }} }},
  "size": {{ "comfortable": {{ "leading-control": 34 }}, "repeat-frame-scale": 1.25 }},
  "layout": {{ "compact": {{ "row-min-height": 52 }}, "three-column-breakpoint": 1100 }},
  "state": {{ "press-scale": 0.98 }}
}}"##
        )
    }

    /// A theme inherits what it does not restate, so a new theme is a handful of overrides.
    #[test]
    fn a_theme_inherits_what_it_extends() {
        let tokens = parse(&source("", "#6B7280"), THEMES).expect("valid");
        let (name, dark) = &tokens.themes[1];
        assert_eq!(name, "dark");
        assert_eq!(dark.appearance, "dark");
        assert_eq!(dark.colours["bg-primary"], Colour::Hex(0x11, 0x18, 0x27));
        assert_eq!(
            dark.colours["accent"],
            Colour::Brand("accent".to_string()),
            "inherited"
        );
    }

    /// Every theme answers for the same names. A token only one theme has is a token a client
    /// will ask another theme for and not get.
    #[test]
    fn a_token_only_one_theme_defines_is_refused() {
        let problems = parse(&source(r##", "glow": "#FFFFFF""##, "#6B7280"), THEMES).unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("themes.dark.colors.glow")),
            "{problems:?}"
        );
    }

    /// The web's AWTD-1496, at the source: grey text chosen for white, on a page that is not.
    #[test]
    fn text_a_theme_cannot_read_is_refused() {
        let problems = parse(&source("", "#9CA3AF"), THEMES).unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("themes.light: text-muted on bg-primary")),
            "{problems:?}"
        );
    }

    /// astrid-web AWTD-1575: one mid-tone per meaning is a fill, not text. `text-red-600` became the
    /// error red and fell from 4.83:1 to 3.76:1 on white. The ink is held to body text's 4.5:1 on
    /// the theme's surfaces and on its own wash.
    #[test]
    fn an_ink_a_theme_cannot_read_is_refused() {
        let with_ink = |ink: &str| {
            source("", "#6B7280").replace(
                r##""accent": "{brand.accent}""##,
                &format!(
                    r##""accent": "{{brand.accent}}", "bg-hover": "#FFFFFF", "error": "#EF4444", "error-ink": "{ink}""##
                ),
            )
        };
        // The fill itself, as text.
        let problems = parse(&with_ink("#EF4444"), THEMES).unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("themes.light: error-ink on bg-hover")),
            "{problems:?}"
        );
        // Readable on white (4.83:1) and not on its own wash (4.14:1).
        let problems = parse(&with_ink("#DC2626"), THEMES).unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("themes.light: error-ink on the error wash over bg-hover")),
            "{problems:?}"
        );
        // Dark inherits it, and a red that reads on white does not on a dark card.
        let problems = parse(&with_ink("#B91C1C"), THEMES).unwrap_err();
        assert!(
            problems
                .iter()
                .all(|p| p.starts_with("themes.dark: error-ink")),
            "{problems:?}"
        );
    }

    /// The accent's ink is the brand's, so a partner replaces it with the accent; the committed
    /// inks are the values the register records.
    #[test]
    fn the_committed_inks_are_readable_and_the_accents_is_the_brands() {
        let root = crate::repo_root();
        let source = std::fs::read_to_string(root.join(SOURCE)).expect("design/tokens.json");
        let tokens = parse(&source, &core_theme_names()).expect("design/tokens.json is valid");
        for (name, theme) in &tokens.themes {
            for meaning in INK_MEANINGS {
                assert!(
                    theme.colours.contains_key(&format!("{meaning}-ink")),
                    "{name} has no {meaning}-ink"
                );
            }
            let want = if theme.appearance == "dark" {
                "accent-ink-dark"
            } else {
                "accent-ink"
            };
            assert_eq!(
                theme.colours["accent-ink"],
                Colour::Brand(want.to_string()),
                "{name}"
            );
        }
        let css = web_css(&tokens);
        assert!(
            css.contains("  --astrid-accent-ink: var(--brand-accent-ink-rgb, 29, 78, 216);"),
            "{css}"
        );
        assert!(
            css.contains("  --astrid-accent-ink: var(--brand-accent-ink-dark-rgb, 96, 165, 250);"),
            "{css}"
        );
    }

    /// astrid-web AWTD-1575 (Jon, 2026-11-11): a button's label is read, so the fill under it is
    /// dark enough for it. White on the old accent `#3B82F6` was 3.68:1 and on the old error red
    /// `#EF4444` 3.76:1.
    #[test]
    fn a_fill_its_label_cannot_be_read_on_is_refused() {
        let with_fills = |accent: &str, hover: &str, error: &str| {
            source("", "#6B7280")
                .replace(
                    r##""brand": { "accent": "#3B82F6" }"##,
                    &format!(
                        r##""brand": {{ "accent": "{accent}", "accent-hover": "{hover}", "accent-text": "#FFFFFF" }}"##
                    ),
                )
                .replace(
                    r##""accent": "{brand.accent}""##,
                    &format!(
                        r##""accent": "{{brand.accent}}", "accent-hover": "{{brand.accent-hover}}", "accent-text": "{{brand.accent-text}}", "error": "{error}""##
                    ),
                )
        };
        let problems = parse(&with_fills("#3B82F6", "#2563EB", "#EF4444"), THEMES).unwrap_err();
        for theme in THEMES {
            for refused in [
                format!("themes.{theme}: accent-text on accent is 3.68:1"),
                format!("themes.{theme}: accent-text on error is 3.76:1"),
            ] {
                assert!(
                    problems.iter().any(|p| p.starts_with(&refused)),
                    "{refused}: {problems:?}"
                );
            }
        }
        assert_eq!(problems.len(), 4, "{problems:?}");
        // A hover lighter than the fill loses the label again.
        let problems = parse(&with_fills("#2563EB", "#3B82F6", "#DC2626"), THEMES).unwrap_err();
        assert!(
            problems
                .iter()
                .all(|p| p.contains("accent-text on accent-hover is 3.68:1")),
            "{problems:?}"
        );
        assert!(parse(&with_fills("#2563EB", "#1D4ED8", "#DC2626"), THEMES).is_ok());
    }

    /// The accent that carries a white label is 2.84:1 as a line on a dark card, so a dark theme's
    /// focus ring is the accent's lighter shade, and one that cannot be seen is refused.
    #[test]
    fn a_focus_ring_a_theme_cannot_show_is_refused() {
        let with_ring = |dark: &str| {
            source(dark, "#6B7280").replace(
                r##""accent": "{brand.accent}""##,
                r##""accent": "{brand.accent}", "border-focus": "#2563EB""##,
            )
        };
        let problems = parse(&with_ring(""), THEMES).unwrap_err();
        assert_eq!(
            problems,
            vec!["themes.dark: border-focus on bg-secondary is 2.84:1, under 3:1".to_string()]
        );
        assert!(parse(&with_ring(r##", "border-focus": "#60A5FA""##), THEMES).is_ok());
    }

    /// The committed fills carry their labels in every theme, and the values are the ones the
    /// register records (DS26).
    #[test]
    fn the_committed_fills_carry_their_labels() {
        let root = crate::repo_root();
        let source = std::fs::read_to_string(root.join(SOURCE)).expect("design/tokens.json");
        let tokens = parse(&source, &core_theme_names()).expect("design/tokens.json is valid");
        assert_eq!(tokens.brand["accent"], (37, 99, 235));
        assert_eq!(tokens.brand["accent-hover"], (29, 78, 216));
        let white = (255, 255, 255);
        for (name, theme) in &tokens.themes {
            assert_eq!(theme.colours["error"], Colour::Hex(220, 38, 38), "{name}");
            assert_eq!(
                theme.colours["accent"],
                Colour::Brand("accent".to_string()),
                "{name}"
            );
            assert_eq!(
                theme.colours["priority-low"],
                Colour::Brand("accent".to_string()),
                "{name}"
            );
            let ring = if theme.appearance == "dark" {
                "accent-ink-dark"
            } else {
                "accent"
            };
            assert_eq!(
                theme.colours["border-focus"],
                Colour::Brand(ring.to_string()),
                "{name}"
            );
        }
        assert!(contrast(white, tokens.brand["accent"]) >= AA_BODY);
        assert!(contrast(white, tokens.brand["accent-hover"]) >= AA_BODY);
        assert!(contrast(white, (220, 38, 38)) >= AA_BODY);
    }

    /// A theme the core cannot name is one nobody can choose.
    #[test]
    fn the_themes_are_the_cores_theme_modes() {
        let problems = parse(&source("", "#6B7280"), &["light", "dark", "ocean"]).unwrap_err();
        assert!(
            problems.iter().any(|p| p.contains("astrid_rules::theme")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_colour_is_a_literal_or_the_brands() {
        assert_eq!(
            Colour::parse("#88dcf8"),
            Some(Colour::Hex(0x88, 0xDC, 0xF8))
        );
        assert_eq!(
            Colour::parse("{brand.accent}"),
            Some(Colour::Brand("accent".to_string()))
        );
        assert_eq!(Colour::parse("#FFF"), None);
        assert_eq!(Colour::parse("red"), None);
    }

    /// The web composes `rgb(var(--x))`, and a partner's accent must still win over Astrid's.
    #[test]
    fn the_web_gets_triplets_and_a_brand_fallback() {
        let tokens = parse(&source("", "#6B7280"), THEMES).expect("valid");
        let css = web_css(&tokens);
        assert!(css.contains(".dark {\n  color-scheme: dark;"), "{css}");
        assert!(css.contains("  --astrid-bg-primary: 17, 24, 39;"), "{css}");
        assert!(
            css.contains("  --astrid-accent: var(--brand-accent-rgb, 59, 130, 246);"),
            "{css}"
        );
        assert!(css.contains("  --astrid-space-4: 4px;"), "{css}");
        assert!(
            css.contains("  --astrid-size-comfortable-leading-control: 34px;"),
            "{css}"
        );
        assert!(
            css.contains("  --astrid-layout-compact-row-min-height: 52px;"),
            "{css}"
        );
        assert!(
            css.contains("  --astrid-layout-three-column-breakpoint: 1100px;"),
            "{css}"
        );
        assert!(css.contains("  --astrid-state-press-scale: 0.98;"), "{css}");
        assert!(
            css.contains("  --astrid-size-repeat-frame-scale: 1.25;"),
            "{css}"
        );
    }

    /// The stamp is a hash of the body, so a consumer can detect a hand edit on its own.
    #[test]
    fn a_generated_file_carries_a_hash_of_its_body() {
        let file = stamped("/*", " */", "body\n");
        let mut lines = file.splitn(3, '\n');
        assert!(lines.next().unwrap().contains("Do not edit."));
        assert_eq!(
            lines.next().unwrap(),
            format!("/* body-sha256: {} */", sha256("body\n"))
        );
        assert_eq!(lines.next().unwrap(), "body\n");
    }

    /// AITD-612: the repeat arrows round someone else's photo or the "U" are the plain square's
    /// own size on every platform (Jon, 2026-11-11). At 1.25 they were bigger than the squares
    /// beside them and stuck out of the column. Reopened the same day: the photo inside must be
    /// the same size as a non-repeating task's photo ("The same size as the default profile
    /// photo"), so the arrows are the photo's border and the inner scale is 1.0, not 0.625.
    #[test]
    fn the_repeat_frame_is_the_size_of_the_square_it_replaces() {
        let root = crate::repo_root();
        let source = std::fs::read_to_string(root.join(SOURCE)).expect("design/tokens.json");
        let json: serde_json::Value = serde_json::from_str(&source).expect("json");
        assert_eq!(json["size"]["repeat-frame-scale"].as_f64(), Some(1.0));
        assert_eq!(json["size"]["repeat-frame-inner-scale"].as_f64(), Some(1.0));
    }

    /// The real file: valid, and already generated. This is what fails a pull request that edits
    /// the source and forgets to regenerate.
    #[test]
    fn the_committed_tokens_are_valid_and_generated() {
        let root = crate::repo_root();
        let source = std::fs::read_to_string(root.join(SOURCE)).expect("design/tokens.json");
        let tokens = parse(&source, &core_theme_names()).expect("design/tokens.json is valid");
        assert_eq!(
            tokens.themes[0].0, "light",
            "the theme the others extend comes first"
        );
        for (relative, expected) in outputs(&tokens) {
            let actual = std::fs::read_to_string(root.join(GENERATED).join(relative))
                .unwrap_or_else(|_| panic!("{relative} is missing; run cargo xtask gen-design"));
            assert_eq!(
                lf(&actual),
                lf(&expected),
                "{relative} is stale; run cargo xtask gen-design"
            );
        }
    }
}
