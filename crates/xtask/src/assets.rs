//! The shared art: the checkbox and the agent marks, one copy for every client (AWTD-1605).
//!
//! Before this, the checkbox was sixteen 56×56 PNGs (`check_box[_repeat][_checked]_{0..3}`), one
//! per priority, copied by hand into astrid-web, the Apple asset catalogs and Windows, which
//! sampled their colours for its priority brushes (register DS2). Fixed-colour bitmaps cannot take
//! the priority tokens or a partner's brand (DS21). The agent marks were copied the same way.
//!
//! `design/assets/` holds the sources and `manifest.json`, which names each file, its sha256 and
//! whose mark it is. The checkbox is four SVGs on the PNGs' own 56-unit canvas, astrid-web's
//! drawing (`components/task-checkbox-art.tsx`): the box and the arrows are `currentColor`, so
//! they take the priority colour; the check is the `checkbox-check` token's green, with its soft
//! shadow. The four are three layers — `box`, `repeat`, `check` — and the generator checks that
//! each variant is exactly its layers, then writes each platform's own kind of resource from them.
//!
//! The checkbox SVGs are a small, fixed vocabulary on purpose: `<g id="layer">` groups of
//! `<path>` elements, a `filter` only for the check's shadow. Anything else is refused rather
//! than silently dropped from the platforms that cannot read SVG.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Map, Value};

use crate::design::{sha256, Colour, Tokens, GENERATED};

/// The sources, relative to the repository root.
pub const ASSETS: &str = "design/assets";

/// The manifest, relative to `ASSETS`.
pub const MANIFEST: &str = "manifest.json";

/// The checkbox variants: (file under `ASSETS`, the layers it is drawn from, bottom first).
const CHECKBOX_VARIANTS: &[(&str, &[&str])] = &[
    ("checkbox/checkbox.svg", &["box"]),
    ("checkbox/checkbox-checked.svg", &["box", "check"]),
    ("checkbox/checkbox-repeat.svg", &["repeat"]),
    ("checkbox/checkbox-repeat-checked.svg", &["repeat", "check"]),
];

/// The layers, in the order a platform draws them.
const LAYERS: &[&str] = &["box", "repeat", "check"];

/// The canvas every checkbox variant is drawn on: the PNGs' own.
const VIEW_BOX: &str = "0 0 56 56";
const CANVAS: u32 = 56;

/// The token the check mark is drawn in.
const CHECK_TOKEN: &str = "checkbox-check";

/// What a path is drawn in.
#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    /// `currentColor`: the priority colour the client sets.
    Tint,
    /// The check mark's green, the `checkbox-check` token.
    Check,
    /// The check's shadow: black at an opacity, blurred where the platform can.
    Shadow { opacity: f64, blur: f64 },
}

/// How a path is painted.
#[derive(Debug, Clone, PartialEq)]
pub enum Paint {
    Fill,
    Stroke(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub role: Role,
    pub paint: Paint,
    pub d: String,
}

/// The checkbox, as layers.
#[derive(Debug, Clone)]
pub struct Checkbox {
    pub layers: BTreeMap<String, Vec<Shape>>,
    /// The check's colour as the SVGs write it, checked against the token.
    pub check: (u8, u8, u8),
}

/// One manifest entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub file: String,
    pub sha256: String,
    pub owner: String,
}

/// A file's hash as the manifest records it: of its text with LF endings, so a Windows checkout
/// with CRLF endings hashes the same.
pub fn file_hash(bytes: &[u8]) -> String {
    sha256(&String::from_utf8_lossy(bytes).replace("\r\n", "\n"))
}

// ── The manifest ─────────────────────────────────────────────────────────────────────────────

/// Read the manifest. Every problem is reported, not just the first.
pub fn parse_manifest(text: &str) -> Result<Vec<Entry>, Vec<String>> {
    let value: Value =
        serde_json::from_str(text).map_err(|e| vec![format!("{MANIFEST} is not JSON: {e}")])?;
    let mut problems = Vec::new();
    if value.get("version").and_then(Value::as_u64) != Some(1) {
        problems.push(format!("{MANIFEST}: version must be 1"));
    }
    let mut entries = Vec::new();
    for (index, item) in value
        .get("assets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let field = |key: &str| {
            item.get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let entry = Entry {
            name: field("name"),
            file: field("file"),
            sha256: field("sha256"),
            owner: field("owner"),
        };
        for (key, text) in [
            ("name", &entry.name),
            ("file", &entry.file),
            ("owner", &entry.owner),
        ] {
            if text.trim().is_empty() {
                problems.push(format!("{MANIFEST}: assets[{index}].{key} is empty"));
            }
        }
        if entry.file.contains("..") || entry.file.starts_with('/') || entry.file.contains('\\') {
            problems.push(format!(
                "{MANIFEST}: assets[{index}].file must be a path under {ASSETS}"
            ));
        }
        if entries.iter().any(|e: &Entry| e.name == entry.name) {
            problems.push(format!(
                "{MANIFEST}: assets[{index}].name {} is listed twice",
                entry.name
            ));
        }
        entries.push(entry);
    }
    if entries.is_empty() {
        problems.push(format!("{MANIFEST}: assets is empty"));
    }
    if problems.is_empty() {
        Ok(entries)
    } else {
        Err(problems)
    }
}

/// The manifest as `gen-design` writes it: the same entries, each hash the file's.
pub fn manifest_text(entries: &[Entry]) -> String {
    let assets: Vec<Value> = entries
        .iter()
        .map(|entry| {
            json!({
                "name": entry.name,
                "file": entry.file,
                "sha256": entry.sha256,
                "owner": entry.owner,
            })
        })
        .collect();
    let mut text = serde_json::to_string_pretty(&json!({ "version": 1, "assets": assets }))
        .expect("a JSON value prints");
    text.push('\n');
    text
}

/// Every file under `ASSETS` but the manifest, relative to it, with `/` separators.
fn files_on_disk(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, base, out);
            } else if let Ok(relative) = path.strip_prefix(base) {
                let relative = relative
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join("/");
                if relative != MANIFEST && !relative.starts_with('.') {
                    out.push(relative);
                }
            }
        }
    }
    let base = root.join(ASSETS);
    let mut out = Vec::new();
    walk(&base, &base, &mut out);
    out.sort();
    out
}

// ── The checkbox SVGs ────────────────────────────────────────────────────────────────────────

/// The attributes of one tag: `name="value"` pairs.
fn attributes(tag: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut rest = tag;
    while let Some(eq) = rest.find("=\"") {
        let name = rest[..eq]
            .rsplit(|c: char| c.is_whitespace() || c == '<')
            .next()
            .unwrap_or_default()
            .to_string();
        let after = &rest[eq + 2..];
        let Some(end) = after.find('"') else { break };
        out.insert(name, after[..end].to_string());
        rest = &after[end + 1..];
    }
    out
}

/// Every `<name ...>` tag in `text`, as its source text.
fn tags<'a>(text: &'a str, name: &str) -> Vec<&'a str> {
    let open = format!("<{name}");
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start..];
        let boundary = after[open.len()..].chars().next();
        let Some(end) = after.find('>') else { break };
        if matches!(boundary, Some(c) if c.is_whitespace() || c == '>' || c == '/') {
            out.push(&after[..=end]);
        }
        rest = &after[end + 1..];
    }
    out
}

fn number(text: &str, what: &str) -> Result<f64, String> {
    text.parse::<f64>()
        .map_err(|_| format!("{what} must be a number, not {text:?}"))
}

/// One checkbox SVG, as `(layer, shapes)` in document order.
pub fn parse_checkbox_svg(file: &str, svg: &str) -> Result<Vec<(String, Vec<Shape>)>, Vec<String>> {
    let mut problems = Vec::new();
    let view_box = tags(svg, "svg")
        .first()
        .and_then(|tag| attributes(tag).get("viewBox").cloned());
    if view_box.as_deref() != Some(VIEW_BOX) {
        problems.push(format!("{file}: the viewBox must be \"{VIEW_BOX}\""));
    }

    // The vocabulary: anything another platform cannot be given is refused, not dropped.
    for element in [
        "rect",
        "circle",
        "ellipse",
        "line",
        "polyline",
        "polygon",
        "text",
        "use",
        "image",
        "linearGradient",
        "radialGradient",
        "mask",
        "clipPath",
    ] {
        if !tags(svg, element).is_empty() {
            problems.push(format!(
                "{file}: <{element}> is not drawn on every platform; write it as a <path>"
            ));
        }
    }
    let blur = tags(svg, "feGaussianBlur")
        .first()
        .and_then(|tag| attributes(tag).get("stdDeviation").cloned());

    let mut layers = Vec::new();
    let mut rest = svg;
    while let Some(start) = rest.find("<g") {
        let after = &rest[start..];
        let Some(close) = after.find("</g>") else {
            problems.push(format!("{file}: a <g> is not closed"));
            break;
        };
        let group = &after[..close];
        rest = &after[close + 4..];
        let Some(head) = tags(group, "g").first().copied() else {
            continue;
        };
        if group[head.len()..].contains("<g") {
            problems.push(format!("{file}: a <g> is nested; layers are one level"));
            continue;
        }
        let head_attributes = attributes(head);
        let Some(id) = head_attributes.get("id").cloned() else {
            problems.push(format!("{file}: a <g> has no id; it names its layer"));
            continue;
        };
        if head_attributes.len() > 1 {
            problems.push(format!(
                "{file}: <g id=\"{id}\"> carries more than its id; put the paint on each <path>"
            ));
        }
        let mut shapes = Vec::new();
        for path in tags(group, "path") {
            match shape(&attributes(path), blur.as_deref()) {
                Ok(shape) => shapes.push(shape),
                Err(why) => problems.push(format!("{file}: layer {id}: {why}")),
            }
        }
        layers.push((id, shapes));
    }
    let in_layers: usize = layers.iter().map(|(_, shapes)| shapes.len()).sum();
    if tags(svg, "path").len() != in_layers {
        problems.push(format!(
            "{file}: every <path> belongs to a <g id=\"layer\">"
        ));
    }
    if problems.is_empty() {
        Ok(layers)
    } else {
        Err(problems)
    }
}

fn shape(attrs: &BTreeMap<String, String>, blur: Option<&str>) -> Result<Shape, String> {
    const KNOWN: &[&str] = &[
        "d",
        "fill",
        "stroke",
        "stroke-width",
        "stroke-opacity",
        "filter",
    ];
    if let Some(unknown) = attrs.keys().find(|key| !KNOWN.contains(&key.as_str())) {
        return Err(format!(
            "a <path> attribute {unknown} is not drawn everywhere"
        ));
    }
    let d = attrs.get("d").cloned().unwrap_or_default();
    if d.trim().is_empty() {
        return Err("a <path> has no d".to_string());
    }
    let fill = attrs.get("fill").map(String::as_str).unwrap_or("none");
    let stroke = attrs.get("stroke").map(String::as_str).unwrap_or("none");
    let (paint, colour) = match (fill, stroke) {
        ("none", "none") => return Err("a <path> paints nothing".to_string()),
        ("none", colour) => {
            let width = attrs
                .get("stroke-width")
                .ok_or("a stroked <path> states its stroke-width")?;
            (Paint::Stroke(number(width, "stroke-width")?), colour)
        }
        (colour, "none") => (Paint::Fill, colour),
        _ => return Err("a <path> is either filled or stroked, not both".to_string()),
    };
    let role = if colour == "currentColor" {
        Role::Tint
    } else if attrs.contains_key("filter") {
        if colour != "#000000" {
            return Err("the shadow is black, at its stroke-opacity".to_string());
        }
        let opacity = number(
            attrs
                .get("stroke-opacity")
                .ok_or("the shadow states its stroke-opacity")?,
            "stroke-opacity",
        )?;
        let blur = number(
            blur.ok_or("the shadow's filter is an feGaussianBlur with a stdDeviation")?,
            "stdDeviation",
        )?;
        Role::Shadow { opacity, blur }
    } else if matches!(Colour::parse(colour), Some(Colour::Hex(..))) {
        Role::Check
    } else {
        return Err(format!(
            "{colour} is neither currentColor nor a #RRGGBB literal"
        ));
    };
    if role == Role::Check && attrs.contains_key("stroke-opacity") {
        return Err("the check is drawn opaque".to_string());
    }
    Ok(Shape { role, paint, d })
}

fn check_colour(shapes: &[Shape], svg: &str) -> Option<(u8, u8, u8)> {
    shapes.iter().find(|shape| shape.role == Role::Check)?;
    tags(svg, "path").into_iter().find_map(|tag| {
        let attrs = attributes(tag);
        if attrs.contains_key("filter") {
            return None;
        }
        let colour = attrs
            .get("stroke")
            .filter(|c| c.as_str() != "none")
            .or_else(|| attrs.get("fill"))?;
        match Colour::parse(colour)? {
            Colour::Hex(r, g, b) => Some((r, g, b)),
            Colour::Brand(_) => None,
        }
    })
}

/// The four variants, checked against each other and the token, as layers.
pub fn checkbox(
    sources: &BTreeMap<String, String>,
    tokens: &Tokens,
) -> Result<Checkbox, Vec<String>> {
    let mut problems = Vec::new();
    let mut layers: BTreeMap<String, (String, Vec<Shape>)> = BTreeMap::new();
    let mut check = None;
    for (file, wanted) in CHECKBOX_VARIANTS {
        let Some(svg) = sources.get(*file) else {
            problems.push(format!("{ASSETS}/{file} is missing"));
            continue;
        };
        let parsed = match parse_checkbox_svg(file, svg) {
            Ok(parsed) => parsed,
            Err(mut errors) => {
                problems.append(&mut errors);
                continue;
            }
        };
        let names: Vec<&str> = parsed.iter().map(|(name, _)| name.as_str()).collect();
        if names != *wanted {
            problems.push(format!(
                "{file}: the layers are {names:?}; this variant is {wanted:?}"
            ));
            continue;
        }
        for (name, shapes) in parsed {
            let roles_ok = match name.as_str() {
                "check" => shapes.iter().all(|s| s.role != Role::Tint),
                _ => shapes.iter().all(|s| s.role == Role::Tint),
            };
            if !roles_ok {
                problems.push(format!(
                    "{file}: layer {name} is drawn in the wrong colour (box and repeat are \
                     currentColor; check is the check green and its shadow)"
                ));
            }
            if name == "check" {
                if let Some(colour) = check_colour(&shapes, svg) {
                    check.get_or_insert(colour);
                }
            }
            match layers.get(&name) {
                Some((first, same)) if *same != shapes => problems.push(format!(
                    "{file}: layer {name} differs from {first}'s; a layer is the same in every variant"
                )),
                Some(_) => {}
                None => {
                    layers.insert(name, (file.to_string(), shapes));
                }
            }
        }
    }

    let token =
        tokens
            .themes
            .first()
            .and_then(|(_, theme)| match theme.colours.get(CHECK_TOKEN)? {
                Colour::Hex(r, g, b) => Some((*r, *g, *b)),
                Colour::Brand(_) => None,
            });
    match (check, token) {
        (Some(drawn), Some(token)) if drawn != token => problems.push(format!(
            "the check is drawn in {} but the {CHECK_TOKEN} token is {}",
            Colour::Hex(drawn.0, drawn.1, drawn.2).source(),
            Colour::Hex(token.0, token.1, token.2).source()
        )),
        (_, None) => problems.push(format!(
            "{CHECK_TOKEN} is not a token; the check is drawn in it"
        )),
        (None, _) if problems.is_empty() => {
            problems.push("the check layer has no check mark".to_string())
        }
        _ => {}
    }

    if problems.is_empty() {
        Ok(Checkbox {
            layers: layers
                .into_iter()
                .map(|(name, (_, shapes))| (name, shapes))
                .collect(),
            check: check.expect("checked above"),
        })
    } else {
        Err(problems)
    }
}

// ── Loading ──────────────────────────────────────────────────────────────────────────────────

/// The sources as read from disk.
pub struct Sources {
    pub entries: Vec<Entry>,
    /// Every listed file's text, keyed by its path under `ASSETS`.
    pub files: BTreeMap<String, String>,
}

/// Read the manifest and every file it lists, and refuse a file on disk it does not list.
pub fn load(root: &Path) -> Result<Sources, Vec<String>> {
    let base = root.join(ASSETS);
    let text = std::fs::read_to_string(base.join(MANIFEST))
        .map_err(|e| vec![format!("cannot read {ASSETS}/{MANIFEST}: {e}")])?;
    let mut entries = parse_manifest(&text)?;
    let mut problems = Vec::new();
    let mut files = BTreeMap::new();
    for entry in &mut entries {
        match std::fs::read(base.join(&entry.file)) {
            Ok(bytes) => {
                entry.sha256 = file_hash(&bytes);
                files.insert(
                    entry.file.clone(),
                    String::from_utf8_lossy(&bytes).replace("\r\n", "\n"),
                );
            }
            Err(e) => problems.push(format!("{ASSETS}/{}: {e}", entry.file)),
        }
    }
    for file in files_on_disk(root) {
        if !entries.iter().any(|entry| entry.file == file) {
            problems.push(format!(
                "{ASSETS}/{file} is not in {MANIFEST}; list it with its owner"
            ));
        }
    }
    for (file, _) in CHECKBOX_VARIANTS {
        if !entries.iter().any(|entry| entry.file == *file) {
            problems.push(format!("{MANIFEST} does not list {file}"));
        }
    }
    if problems.is_empty() {
        Ok(Sources { entries, files })
    } else {
        Err(problems)
    }
}

// ── Emitters ─────────────────────────────────────────────────────────────────────────────────

/// Every file the assets generate: (path relative to the repository root, contents). The
/// manifest is first: `gen-design` rewrites its hashes.
pub fn outputs(sources: &Sources, tokens: &Tokens) -> Result<Vec<(String, String)>, Vec<String>> {
    let checkbox = checkbox(&sources.files, tokens)?;
    let mut out = vec![
        (
            format!("{ASSETS}/{MANIFEST}"),
            manifest_text(&sources.entries),
        ),
        (
            format!("{GENERATED}/web/checkbox-art.json"),
            web_json(&checkbox),
        ),
    ];
    let catalog = format!("{GENERATED}/apple/AstridDesign.xcassets");
    out.push((
        format!("{catalog}/Contents.json"),
        pretty(&json!({ "info": { "author": "xcode", "version": 1 } })),
    ));
    for layer in LAYERS {
        let name = format!("checkbox-{layer}");
        let template = *layer != "check";
        out.push((
            format!("{catalog}/{name}.imageset/Contents.json"),
            imageset(&format!("{name}.svg"), template),
        ));
        out.push((
            format!("{catalog}/{name}.imageset/{name}.svg"),
            xml_stamped(&layer_svg(&checkbox, layer)),
        ));
        out.push((
            format!("{GENERATED}/android/drawable/checkbox_{layer}.xml"),
            xml_stamped(&vector_drawable(&checkbox, layer)),
        ));
    }
    for entry in &sources.entries {
        let Some(stem) = entry
            .file
            .strip_prefix("agents/")
            .and_then(|f| f.strip_suffix(".svg"))
        else {
            continue;
        };
        let name = format!("agent-{stem}");
        out.push((
            format!("{catalog}/{name}.imageset/Contents.json"),
            imageset(&format!("{name}.svg"), false),
        ));
        out.push((
            format!("{catalog}/{name}.imageset/{name}.svg"),
            sources.files[&entry.file].clone(),
        ));
    }
    out.push((
        format!("{GENERATED}/windows/AstridCheckbox.xaml"),
        xml_stamped(&xaml(&checkbox)),
    ));
    Ok(out)
}

fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).expect("a JSON value prints");
    text.push('\n');
    text
}

/// The `body-sha256` stamp, as an XML comment.
fn xml_stamped(body: &str) -> String {
    format!(
        "<!-- GENERATED by `cargo xtask gen-design` from astrid-core design/assets. Do not edit. -->\n\
         <!-- body-sha256: {} -->\n{body}",
        sha256(body)
    )
}

fn hex((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// A number as a platform writes it: no trailing `.0`.
fn num(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{n:.0}")
    } else {
        format!("{n}")
    }
}

/// The shapes as data, for a client that draws them itself: astrid-web's checkbox, which keeps
/// them inline so a list of tasks loads no images. `tint` is the priority colour (`currentColor`),
/// `check` the `checkbox-check` token, `shadow` black at its opacity and blur.
fn web_json(checkbox: &Checkbox) -> String {
    let mut layers = Map::new();
    for layer in LAYERS {
        let shapes: Vec<Value> = checkbox.layers[*layer]
            .iter()
            .map(|shape| {
                let mut out = Map::new();
                let (role, extra) = match &shape.role {
                    Role::Tint => ("tint", None),
                    Role::Check => ("check", None),
                    Role::Shadow { opacity, blur } => ("shadow", Some((*opacity, *blur))),
                };
                out.insert("role".into(), json!(role));
                out.insert("d".into(), json!(shape.d));
                match shape.paint {
                    Paint::Fill => {
                        out.insert("paint".into(), json!("fill"));
                    }
                    Paint::Stroke(width) => {
                        out.insert("paint".into(), json!("stroke"));
                        out.insert("width".into(), json_number(width));
                    }
                }
                if let Some((opacity, blur)) = extra {
                    out.insert("opacity".into(), json_number(opacity));
                    out.insert("blur".into(), json_number(blur));
                }
                Value::Object(out)
            })
            .collect();
        layers.insert((*layer).to_string(), Value::Array(shapes));
    }
    pretty(&json!({
        "viewBox": VIEW_BOX,
        "checkColor": hex(checkbox.check),
        "layers": layers,
    }))
}

/// A number as JSON, whole numbers without a `.0`.
fn json_number(n: f64) -> Value {
    if n.fract() == 0.0 {
        json!(n as i64)
    } else {
        json!(n)
    }
}

fn imageset(file: &str, template: bool) -> String {
    let mut properties = Map::new();
    properties.insert("preserves-vector-representation".into(), json!(true));
    properties.insert(
        "template-rendering-intent".into(),
        json!(if template { "template" } else { "original" }),
    );
    pretty(&json!({
        "images": [{ "filename": file, "idiom": "universal" }],
        "info": { "author": "xcode", "version": 1 },
        "properties": properties,
    }))
}

/// One layer as a plain SVG with no filter: an asset catalog draws a template in its tint from
/// the alpha alone, so the tinted layers are black, and the shadow is drawn unblurred (0.8 of a
/// 56-unit canvas is under a pixel at any size the checkbox is shown).
fn layer_svg(checkbox: &Checkbox, layer: &str) -> String {
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{VIEW_BOX}\" width=\"{CANVAS}\" height=\"{CANVAS}\">\n"
    );
    for shape in &checkbox.layers[layer] {
        let (colour, opacity) = match &shape.role {
            Role::Tint => ("#000000".to_string(), None),
            Role::Check => (hex(checkbox.check), None),
            Role::Shadow { opacity, .. } => ("#000000".to_string(), Some(*opacity)),
        };
        let paint = match shape.paint {
            Paint::Fill => format!("fill=\"{colour}\""),
            Paint::Stroke(width) => format!(
                "fill=\"none\" stroke=\"{colour}\" stroke-width=\"{}\" stroke-miterlimit=\"4\"",
                num(width)
            ),
        };
        let opacity = opacity
            .map(|o| format!(" opacity=\"{}\"", num(o)))
            .unwrap_or_default();
        svg.push_str(&format!("  <path d=\"{}\" {paint}{opacity}/>\n", shape.d));
    }
    svg.push_str("</svg>\n");
    svg
}

/// An Android vector drawable. The tinted layers are white, for `android:tint` or Compose's
/// `Icon(tint = …)`; the check layer is drawn in its own colours and is not tinted.
fn vector_drawable(checkbox: &Checkbox, layer: &str) -> String {
    let mut xml = format!(
        "<vector xmlns:android=\"http://schemas.android.com/apk/res/android\"\n    \
         android:width=\"{CANVAS}dp\"\n    android:height=\"{CANVAS}dp\"\n    \
         android:viewportWidth=\"{CANVAS}\"\n    android:viewportHeight=\"{CANVAS}\">\n"
    );
    for shape in &checkbox.layers[layer] {
        let (colour, alpha) = match &shape.role {
            Role::Tint => ("#FFFFFFFF".to_string(), None),
            Role::Check => (format!("#FF{}", &hex(checkbox.check)[1..]), None),
            Role::Shadow { opacity, .. } => ("#FF000000".to_string(), Some(*opacity)),
        };
        xml.push_str(&format!(
            "    <path\n        android:pathData=\"{}\"",
            shape.d
        ));
        match shape.paint {
            Paint::Fill => {
                xml.push_str(&format!("\n        android:fillColor=\"{colour}\""));
                if let Some(alpha) = alpha {
                    xml.push_str(&format!("\n        android:fillAlpha=\"{}\"", num(alpha)));
                }
            }
            Paint::Stroke(width) => {
                xml.push_str(&format!(
                    "\n        android:strokeColor=\"{colour}\"\n        android:strokeWidth=\"{}\"\n        android:strokeMiterLimit=\"4\"",
                    num(width)
                ));
                if let Some(alpha) = alpha {
                    xml.push_str(&format!("\n        android:strokeAlpha=\"{}\"", num(alpha)));
                }
            }
        }
        xml.push_str(" />\n");
    }
    xml.push_str("</vector>\n");
    xml
}

/// A WinUI resource dictionary: one `ContentControl` style per layer. The box and the arrows
/// take the control's `Foreground`, so Windows sets the priority brush instead of sampling a
/// bitmap (register DS2); a checked checkbox stacks the check over its box in a `Grid`.
fn xaml(checkbox: &Checkbox) -> String {
    let mut xml = String::from(
        "<ResourceDictionary\n    xmlns=\"http://schemas.microsoft.com/winfx/2006/xaml/presentation\"\n    \
         xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\">\n\n",
    );
    xml.push_str(&format!(
        "    <SolidColorBrush x:Key=\"AstridCheckboxCheckBrush\" Color=\"{}\" />\n",
        hex(checkbox.check)
    ));
    for layer in LAYERS {
        let style = format!("AstridCheckbox{}Style", capitalised(layer));
        xml.push_str(&format!(
            "\n    <Style x:Key=\"{style}\" TargetType=\"ContentControl\">\n        \
             <Setter Property=\"IsTabStop\" Value=\"False\" />\n        \
             <Setter Property=\"Template\">\n            <Setter.Value>\n                \
             <ControlTemplate TargetType=\"ContentControl\">\n                    \
             <Viewbox>\n                        <Canvas Width=\"{CANVAS}\" Height=\"{CANVAS}\">\n"
        ));
        for shape in &checkbox.layers[*layer] {
            let (brush, opacity) = match &shape.role {
                Role::Tint => ("{TemplateBinding Foreground}".to_string(), None),
                Role::Check => (
                    "{StaticResource AstridCheckboxCheckBrush}".to_string(),
                    None,
                ),
                Role::Shadow { opacity, .. } => ("Black".to_string(), Some(*opacity)),
            };
            let paint = match shape.paint {
                Paint::Fill => format!("Fill=\"{brush}\""),
                Paint::Stroke(width) => format!(
                    "Stroke=\"{brush}\" StrokeThickness=\"{}\" StrokeMiterLimit=\"4\"",
                    num(width)
                ),
            };
            let opacity = opacity
                .map(|o| format!(" Opacity=\"{}\"", num(o)))
                .unwrap_or_default();
            xml.push_str(&format!(
                "                            <Path Data=\"{}\" {paint}{opacity} />\n",
                shape.d
            ));
        }
        xml.push_str(
            "                        </Canvas>\n                    </Viewbox>\n                \
             </ControlTemplate>\n            </Setter.Value>\n        </Setter>\n    </Style>\n",
        );
    }
    xml.push_str("</ResourceDictionary>\n");
    xml
}

fn capitalised(name: &str) -> String {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn committed() -> (Sources, Tokens) {
        let root = crate::repo_root();
        let source =
            std::fs::read_to_string(root.join(crate::design::SOURCE)).expect("design/tokens.json");
        let tokens = crate::design::parse(&source, &crate::design::core_theme_names())
            .expect("design/tokens.json is valid");
        let sources = load(&root).unwrap_or_else(|problems| panic!("{problems:?}"));
        (sources, tokens)
    }

    /// AWTD-1605: the four variants are three layers, the box and arrows in the priority colour
    /// and the check in the token's green, and every platform output is current.
    #[test]
    fn the_committed_assets_are_valid_and_generated() {
        let root = crate::repo_root();
        let (sources, tokens) = committed();
        let checkbox = checkbox(&sources.files, &tokens).unwrap_or_else(|p| panic!("{p:?}"));
        assert_eq!(
            checkbox.layers.keys().collect::<Vec<_>>(),
            vec!["box", "check", "repeat"]
        );
        assert_eq!(checkbox.check, (0x89, 0xA9, 0x12));
        for (relative, expected) in outputs(&sources, &tokens).expect("assets are valid") {
            let actual = std::fs::read_to_string(root.join(&relative))
                .unwrap_or_else(|_| panic!("{relative} is missing; run cargo xtask gen-design"));
            assert_eq!(
                actual.replace("\r\n", "\n"),
                expected,
                "{relative} is stale; run cargo xtask gen-design"
            );
        }
    }

    /// The agent marks are the files astrid-web shipped, each with an owner.
    #[test]
    fn every_asset_has_an_owner_and_a_hash() {
        let (sources, _) = committed();
        let names: Vec<&str> = sources.entries.iter().map(|e| e.name.as_str()).collect();
        for agent in ["claude", "copilot", "gemini", "muse", "openai", "openclaw"] {
            assert!(
                names.contains(&format!("agent-{agent}").as_str()),
                "{names:?}"
            );
        }
        for entry in &sources.entries {
            assert_eq!(entry.sha256.len(), 64, "{}", entry.name);
            assert!(!entry.owner.is_empty(), "{}", entry.name);
        }
    }

    fn variants(edit: impl Fn(&str, String) -> String) -> BTreeMap<String, String> {
        let (sources, _) = committed();
        sources
            .files
            .into_iter()
            .filter(|(file, _)| file.starts_with("checkbox/"))
            .map(|(file, text)| {
                let edited = edit(&file, text);
                (file, edited)
            })
            .collect()
    }

    /// A green that is not the token's is the art drifting from the tokens again.
    #[test]
    fn a_check_not_in_the_token_green_is_refused() {
        let (_, tokens) = committed();
        let files = variants(|_, text| text.replace("#89A912", "#A3C61A"));
        let problems = checkbox(&files, &tokens).unwrap_err();
        assert_eq!(
            problems,
            vec![
                "the check is drawn in #A3C61A but the checkbox-check token is #89A912".to_string()
            ]
        );
    }

    /// A layer is one drawing: the box of the checked variant is the plain box.
    #[test]
    fn a_layer_that_differs_between_variants_is_refused() {
        let (_, tokens) = committed();
        let files = variants(|file, text| {
            if file == "checkbox/checkbox-checked.svg" {
                text.replace("stroke-width=\"4\"", "stroke-width=\"5\"")
            } else {
                text
            }
        });
        let problems = checkbox(&files, &tokens).unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("layer box differs from checkbox/checkbox.svg's")),
            "{problems:?}"
        );
    }

    /// The box takes the priority colour; a fixed colour there is DS21 again.
    #[test]
    fn a_box_in_a_fixed_colour_is_refused() {
        let (_, tokens) = committed();
        let files = variants(|_, text| text.replace("currentColor", "#328ACC"));
        let problems = checkbox(&files, &tokens).unwrap_err();
        assert!(
            problems.iter().any(|p| p.contains("wrong colour")),
            "{problems:?}"
        );
    }

    /// What Android and Windows cannot be given is refused, not dropped.
    #[test]
    fn an_element_outside_the_vocabulary_is_refused() {
        let svg = "<svg viewBox=\"0 0 56 56\"><g id=\"box\"><rect x=\"1\"/></g></svg>";
        let problems = parse_checkbox_svg("x.svg", svg).unwrap_err();
        assert!(
            problems.iter().any(|p| p.contains("<rect>")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_file_hash_ignores_line_endings() {
        assert_eq!(file_hash(b"a\r\nb\n"), file_hash(b"a\nb\n"));
    }

    #[test]
    fn a_manifest_entry_needs_its_fields() {
        let problems = parse_manifest(
            r#"{ "version": 1, "assets": [{ "name": "x", "file": "../x.svg", "sha256": "", "owner": "" }] }"#,
        )
        .unwrap_err();
        assert!(
            problems.iter().any(|p| p.contains("owner is empty")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("must be a path under")),
            "{problems:?}"
        );
    }
}
