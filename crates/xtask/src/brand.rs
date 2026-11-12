//! One brand profile, and every client's brand resources written from it (task 77367032).
//!
//! Before this there were four formats and no schema: web's `brands/*.brand.json` carried an `env`
//! bag of 26 `NEXT_PUBLIC_*` keys validated in TypeScript, iOS read that profile plus an `ios`
//! block mapped by hand in Swift, Windows kept a *fork* of the same file with five of the keys, and
//! Android used `brand.properties`, a different format with fourteen values re-typed. A partner
//! needed roughly seventeen steps on web, fourteen on Apple, ten on Windows and nine on Android,
//! several of them edits to source.
//!
//! `brand/schema.json` is the one description, in sections, and it carries the `env` key each field
//! is ALSO readable as. That alias is what lets this land without a migration: every profile in
//! astrid-web's `brands/` validates unchanged today, and a client moves to the sections when it
//! moves, rather than everything moving at once or nothing moving at all.
//!
//! The emitters here write TEXT. Icons are deliberately not among them — see `assets.rs`, which
//! exists because fixed-colour bitmaps cannot take the priority tokens or a partner's brand, and
//! which has no rasteriser for exactly that reason. Resizing a 1024px icon is a new dependency and
//! a supply-chain decision, so it is its own task rather than a quiet addition here.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use serde_json::{Map, Value};

use crate::design::sha256;

/// The schema, relative to the repository root.
pub const SCHEMA: &str = "brand/schema.json";

/// What a field holds, and so what makes it invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Str,
    Colour,
    Email,
    Domain,
    Scheme,
    Bool,
    List,
    Path,
}

impl Kind {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "string" => Kind::Str,
            "color" => Kind::Colour,
            "email" => Kind::Email,
            "domain" => Kind::Domain,
            "scheme" => Kind::Scheme,
            "bool" => Kind::Bool,
            "list" => Kind::List,
            "path" => Kind::Path,
            _ => return None,
        })
    }

    /// Why this value is not one of these, or `None` when it is.
    fn problem(self, value: &str) -> Option<String> {
        match self {
            Kind::Str | Kind::Path | Kind::List => None,
            Kind::Colour => (!is_hex6(value)).then(|| format!("{value:?} is not a #rrggbb colour")),
            Kind::Email => (!value.contains('@') || !value.contains('.'))
                .then(|| format!("{value:?} is not an email address")),
            Kind::Domain => (!value.contains('.') || value.contains('/') || value.contains('@'))
                .then(|| format!("{value:?} is not a bare domain")),
            // A URL scheme reaches the OS: a space or a colon in it produces a registration that
            // silently never matches, on every platform at once.
            Kind::Scheme => (value.is_empty()
                || !value
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()))
            .then(|| format!("{value:?} is not a url scheme (lower-case letters and digits)")),
            Kind::Bool => (!matches!(value, "true" | "false"))
                .then(|| format!("{value:?} is not true or false")),
        }
    }
}

/// One field: where it lives, what it holds, and whether a profile must say it.
#[derive(Debug, Clone)]
struct Field {
    kind: Kind,
    required: bool,
    /// The `NEXT_PUBLIC_*`/`BRAND_*` key this field is also readable as.
    env: Option<String>,
    /// Astrid's own value, used when a profile states none. This is what lets
    /// `astrid.brand.json` set nothing at all and still describe Astrid.
    default: Option<String>,
}

/// `brand/schema.json`, parsed.
#[derive(Debug, Clone)]
pub struct Schema {
    version: u64,
    /// Section name -> field name -> field. Ordered, so errors and outputs are stable.
    sections: BTreeMap<String, BTreeMap<String, Field>>,
    platforms: BTreeMap<String, BTreeMap<String, Field>>,
}

/// A profile, normalised onto the schema's sections.
#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub name: String,
    pub description: String,
    /// `section.field` -> value, every value as the string the emitters write.
    pub values: BTreeMap<String, String>,
}

impl Profile {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// The value, or the empty string — for the many optional fields an emitter simply omits.
    fn or_empty(&self, key: &str) -> &str {
        self.get(key).unwrap_or_default()
    }
}

fn is_hex6(value: &str) -> bool {
    let Some(body) = value.strip_prefix('#') else {
        return false;
    };
    body.len() == 6 && body.chars().all(|c| c.is_ascii_hexdigit())
}

fn fields(raw: &Value, what: &str) -> Result<BTreeMap<String, Field>, Vec<String>> {
    let mut problems = Vec::new();
    let mut out = BTreeMap::new();
    let Some(object) = raw.as_object() else {
        return Err(vec![format!("{what} is not an object")]);
    };

    for (name, spec) in object {
        let kind = spec.get("type").and_then(Value::as_str).unwrap_or("string");
        match Kind::parse(kind) {
            Some(kind) => {
                out.insert(
                    name.clone(),
                    Field {
                        kind,
                        required: spec
                            .get("required")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        env: spec.get("env").and_then(Value::as_str).map(str::to_string),
                        default: spec
                            .get("default")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    },
                );
            }
            None => problems.push(format!("{what}.{name}: unknown type {kind:?}")),
        }
    }

    if problems.is_empty() {
        Ok(out)
    } else {
        Err(problems)
    }
}

impl Schema {
    pub fn parse(source: &str) -> Result<Self, Vec<String>> {
        let raw: Value = serde_json::from_str(source)
            .map_err(|error| vec![format!("brand/schema.json is not JSON: {error}")])?;

        let version = raw.get("version").and_then(Value::as_u64).unwrap_or(0);
        if version == 0 {
            return Err(vec!["brand/schema.json has no version".to_string()]);
        }

        let mut problems = Vec::new();
        let mut sections = BTreeMap::new();
        let mut platforms = BTreeMap::new();

        for (group, into) in [("sections", &mut sections), ("platforms", &mut platforms)] {
            let Some(object) = raw.get(group).and_then(Value::as_object) else {
                problems.push(format!("brand/schema.json has no {group}"));
                continue;
            };
            for (name, spec) in object {
                match fields(spec, &format!("{group}.{name}")) {
                    Ok(parsed) => {
                        into.insert(name.clone(), parsed);
                    }
                    Err(mut errors) => problems.append(&mut errors),
                }
            }
        }

        if problems.is_empty() {
            Ok(Schema {
                version,
                sections,
                platforms,
            })
        } else {
            Err(problems)
        }
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Every field, as `("section.field", field)`, sections then platforms.
    fn all(&self) -> impl Iterator<Item = (String, &Field)> {
        self.sections
            .iter()
            .chain(self.platforms.iter())
            .flat_map(|(section, fields)| {
                fields
                    .iter()
                    .map(move |(name, field)| (format!("{section}.{name}"), field))
            })
    }

    /// Read a profile against this schema: sections first, then the `env` alias, then validate.
    ///
    /// Every problem names its field as `section.field`, because the point of the schema is that a
    /// partner's broken profile says which line to fix rather than failing somewhere downstream in
    /// a generated file.
    /// With no defaults beyond the schema's own. Every caller in the binary has Astrid's colours
    /// to pass, so this exists for the tests that are checking the schema rather than the tokens.
    #[cfg(test)]
    pub fn read(&self, source: &str) -> Result<Profile, Vec<String>> {
        self.read_with(source, &BTreeMap::new())
    }

    /// As [`Schema::read`], with `defaults` overlaying the schema's own — `gen-brand` passes the
    /// `brand` block of `design/tokens.json`, so Astrid's accent has ONE home rather than being
    /// restated in the schema and drifting from the tokens the way the other four copies did.
    pub fn read_with(
        &self,
        source: &str,
        defaults: &BTreeMap<String, String>,
    ) -> Result<Profile, Vec<String>> {
        let raw: Value = serde_json::from_str(source)
            .map_err(|error| vec![format!("profile is not JSON: {error}")])?;
        let Some(object) = raw.as_object() else {
            return Err(vec!["profile is not an object".to_string()]);
        };

        let env = object
            .get("env")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();

        let mut problems = Vec::new();
        let mut values = BTreeMap::new();

        for (key, field) in self.all() {
            let (section, name) = key.split_once('.').expect("built with a dot");
            let from_section = object
                .get(section)
                .and_then(Value::as_object)
                .and_then(|s| s.get(name))
                .map(scalar);
            let from_env = field
                .env
                .as_ref()
                .and_then(|alias| env.get(alias))
                .map(scalar);

            // A section wins over its alias: a profile that has migrated says so by having the
            // section, and leaving the old key behind must not silently override the new one.
            let fallback = defaults
                .get(&key)
                .cloned()
                .or_else(|| field.default.clone());

            let Some(value) = from_section.or(from_env).or(fallback) else {
                if field.required {
                    problems.push(match &field.env {
                        Some(alias) => format!("{key} is required (or env.{alias})"),
                        None => format!("{key} is required"),
                    });
                }
                continue;
            };

            // An empty optional is absent, not invalid: web's profiles carry "" for unset keys.
            if value.is_empty() {
                if field.required {
                    problems.push(format!("{key} is required and is empty"));
                }
                continue;
            }

            if let Some(problem) = field.kind.problem(&value) {
                problems.push(format!("{key}: {problem}"));
                continue;
            }

            values.insert(key, value);
        }

        let name = object
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if name.is_empty() {
            problems.push("name is required".to_string());
        }

        // The accent is checked, and the colours that follow from it filled in, by the one rule
        // every client shares (task 5ea48ab1). A stated value always wins: Astrid's own hover and
        // inks are hand-picked on a designed ramp that no formula reproduces, and derivation is
        // for a partner who supplies an accent and nothing else.
        if let Some(accent) = values.get("colors.accent").cloned() {
            match astrid_rules::brand_colors::derive(&accent) {
                Ok(derived) => {
                    for (key, value) in [
                        ("colors.accent-hover", derived.accent_hover),
                        ("colors.accent-ink", derived.accent_ink),
                        ("colors.accent-ink-dark", derived.accent_ink_dark),
                        ("colors.accent-text", derived.accent_text),
                    ] {
                        values.entry(key.to_string()).or_insert(value);
                    }
                }
                Err(refusal) => problems.push(format!("colors.accent: {}", refusal.reason)),
            }
        }

        if problems.is_empty() {
            Ok(Profile {
                name,
                description: object
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                values,
            })
        } else {
            problems.sort();
            Err(problems)
        }
    }
}

/// A scalar as the string the emitters write; a list joins on commas, as the env keys already do.
fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::Array(items) => items.iter().map(scalar).collect::<Vec<_>>().join(","),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A stable id for a platform that needs one, derived from the bundle id rather than invented.
///
/// Astrid Code wants GUIDs that must not move between runs or machines — a regenerated id orphans
/// every installed copy's settings. So they are a hash of the bundle id and the id's own purpose,
/// formatted as a v4-shaped UUID, which is reproducible from the profile alone.
pub fn derived_uuid(bundle_id: &str, purpose: &str) -> String {
    let digest = sha256(&format!("{bundle_id}/{purpose}"));
    format!(
        "{}-{}-4{}-{}{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[13..16],
        // The variant nibble, so the shape is a valid UUID rather than merely 32 hex characters.
        ['8', '9', 'a', 'b'][usize::from(u8::from_str_radix(&digest[16..17], 16).unwrap_or(0) % 4)],
        &digest[17..20],
        &digest[20..32]
    )
}

fn env_line(key: &str, value: &str) -> String {
    format!("{key}={value}\n")
}

/// Everything `gen-brand` writes, as (path under the output directory, contents).
pub fn outputs(schema: &Schema, profile: &Profile) -> Vec<(String, String)> {
    vec![
        ("web/.env.brand".to_string(), web_env(schema, profile)),
        ("web/brand.css".to_string(), web_css(profile)),
        ("apple/Brand.xcconfig".to_string(), apple_xcconfig(profile)),
        ("apple/BrandInfo.plist".to_string(), apple_plist(profile)),
        (
            "android/brand.properties".to_string(),
            android_properties(profile),
        ),
        ("android/colors.xml".to_string(), android_colours(profile)),
        ("windows/Brand.xaml".to_string(), windows_xaml(profile)),
        (
            "windows/brand.manifest.json".to_string(),
            windows_manifest(profile),
        ),
        (
            "vscode/package.brand.json".to_string(),
            vscode_json(profile),
        ),
        ("code/product.patch.json".to_string(), code_product(profile)),
    ]
}

/// The header every generated file carries, in that file's comment syntax.
fn banner(prefix: &str, profile: &Profile) -> String {
    let mut text = format!(
        "{prefix} Generated by `cargo xtask gen-brand` from the {} brand profile.\n",
        profile.name
    );
    // The profile's own description too, because several deployments can share a brand and a
    // generated file that says only "Acme" does not say WHICH Acme it was written for.
    if !profile.description.is_empty() {
        text.push_str(&format!("{prefix} {}\n", profile.description));
    }
    text.push_str(prefix);
    text.push_str(" Do not edit: change the profile and regenerate.\n");
    text
}

fn web_env(schema: &Schema, profile: &Profile) -> String {
    let mut text = banner("#", profile);
    // Web still reads the env keys, so this writes exactly the aliases the schema declares —
    // which is what keeps the generated environment and today's hand-written one the same shape.
    for (key, field) in schema.all() {
        let (Some(alias), Some(value)) = (field.env.as_ref(), profile.get(&key)) else {
            continue;
        };
        text.push_str(&env_line(alias, value));
    }
    text
}

fn web_css(profile: &Profile) -> String {
    // One block comment with continuation lines, rather than a `/*` on each: nested `/*` is legal
    // CSS but reads as a mistake in a file somebody is meant to be able to open.
    let mut text = String::from("/*\n");
    text.push_str(&banner(" *", profile));
    text.push_str(" */\n:root {\n");
    for (token, key) in [
        ("--brand-accent", "colors.accent"),
        ("--brand-accent-hover", "colors.accent-hover"),
        ("--brand-accent-ink", "colors.accent-ink"),
        ("--brand-accent-ink-dark", "colors.accent-ink-dark"),
    ] {
        if let Some(value) = profile.get(key) {
            text.push_str(&format!("  {token}: {value};\n"));
        }
    }
    if let Some(family) = profile.get("font.family") {
        text.push_str(&format!("  --brand-font-family: {family};\n"));
    }
    text.push_str("}\n");
    text
}

fn apple_xcconfig(profile: &Profile) -> String {
    let mut text = banner("//", profile);
    for (setting, key) in [
        ("BRAND_NAME", "identity.name"),
        ("BRAND_DISPLAY_NAME", "ios.display-name"),
        ("PRODUCT_BUNDLE_IDENTIFIER", "ios.bundle-id"),
        ("DEVELOPMENT_TEAM", "ios.development-team"),
        ("BRAND_APP_GROUP", "ios.app-group"),
        ("BRAND_KEYCHAIN_GROUP", "ios.keychain-group"),
        ("BRAND_URL_SCHEME", "identity.url-scheme"),
        ("BRAND_GOOGLE_CLIENT_ID", "auth.google-client-id"),
    ] {
        if let Some(value) = profile.get(key) {
            text.push_str(&format!("{setting} = {value}\n"));
        }
    }
    text
}

fn apple_plist(profile: &Profile) -> String {
    let scheme = profile.or_empty("identity.url-scheme");
    let mut text = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    text.push_str(&banner("<!--", profile).replace('\n', " -->\n"));
    text.push_str("<plist version=\"1.0\">\n<dict>\n");
    text.push_str(&format!(
        "  <key>CFBundleDisplayName</key>\n  <string>{}</string>\n",
        xml(profile.or_empty("identity.title"))
    ));
    text.push_str("  <key>CFBundleURLTypes</key>\n  <array>\n    <dict>\n");
    text.push_str("      <key>CFBundleURLSchemes</key>\n      <array>\n");
    text.push_str(&format!("        <string>{}</string>\n", xml(scheme)));
    text.push_str("      </array>\n    </dict>\n  </array>\n</dict>\n</plist>\n");
    text
}

fn android_properties(profile: &Profile) -> String {
    let mut text = banner("#", profile);
    for (property, key) in [
        ("brand.name", "identity.name"),
        ("brand.title", "identity.title"),
        ("brand.package", "android.package"),
        ("brand.domain", "identity.domain"),
        ("brand.urlScheme", "identity.url-scheme"),
        ("brand.accent", "colors.accent"),
        ("brand.supportEmail", "identity.support-email"),
    ] {
        if let Some(value) = profile.get(key) {
            text.push_str(&format!("{property}={value}\n"));
        }
    }
    text
}

fn android_colours(profile: &Profile) -> String {
    let mut text = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    text.push_str(&banner("<!--", profile).replace('\n', " -->\n"));
    text.push_str("<resources>\n");
    for (name, key) in [
        ("brand_accent", "colors.accent"),
        ("brand_accent_hover", "colors.accent-hover"),
        ("brand_accent_ink", "colors.accent-ink"),
        ("brand_accent_ink_dark", "colors.accent-ink-dark"),
    ] {
        if let Some(value) = profile.get(key) {
            text.push_str(&format!(
                "  <color name=\"{name}\">{}</color>\n",
                value.to_uppercase()
            ));
        }
    }
    text.push_str("</resources>\n");
    text
}

fn windows_xaml(profile: &Profile) -> String {
    let mut text = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    text.push_str(&banner("<!--", profile).replace('\n', " -->\n"));
    text.push_str(
        "<ResourceDictionary\n    \
         xmlns=\"http://schemas.microsoft.com/winfx/2006/xaml/presentation\"\n    \
         xmlns:x=\"http://schemas.microsoft.com/winfx/2006/xaml\">\n",
    );
    for (key_name, key) in [
        ("BrandAccentColor", "colors.accent"),
        ("BrandAccentHoverColor", "colors.accent-hover"),
        ("BrandAccentInkColor", "colors.accent-ink"),
        ("BrandAccentInkDarkColor", "colors.accent-ink-dark"),
    ] {
        if let Some(value) = profile.get(key) {
            text.push_str(&format!(
                "    <Color x:Key=\"{key_name}\">{}</Color>\n",
                value.to_uppercase()
            ));
        }
    }
    if let Some(name) = profile.get("identity.name") {
        text.push_str(&format!(
            "    <x:String x:Key=\"BrandName\">{}</x:String>\n",
            xml(name)
        ));
    }
    text.push_str("</ResourceDictionary>\n");
    text
}

fn windows_manifest(profile: &Profile) -> String {
    let mut manifest = Map::new();
    for (name, key) in [
        ("identity", "windows.package-identity"),
        ("publisher", "windows.publisher"),
        ("displayName", "windows.display-name"),
        ("protocol", "identity.url-scheme"),
    ] {
        if let Some(value) = profile.get(key) {
            manifest.insert(name.to_string(), Value::String(value.to_string()));
        }
    }
    pretty(&Value::Object(manifest))
}

fn vscode_json(profile: &Profile) -> String {
    let mut package = Map::new();
    if let Some(id) = profile.get("vscode.extension-id") {
        package.insert("name".to_string(), Value::String(id.to_string()));
    }
    if let Some(publisher) = profile.get("vscode.publisher") {
        package.insert(
            "publisher".to_string(),
            Value::String(publisher.to_string()),
        );
    }
    if let Some(title) = profile.get("identity.title") {
        package.insert("displayName".to_string(), Value::String(title.to_string()));
    }
    if let Some(tagline) = profile.get("identity.tagline") {
        package.insert(
            "description".to_string(),
            Value::String(tagline.to_string()),
        );
    }
    pretty(&Value::Object(package))
}

fn code_product(profile: &Profile) -> String {
    let bundle = profile.or_empty("code.bundle-id");
    let mut product = Map::new();
    if let Some(name) = profile.get("code.application-name") {
        product.insert(
            "applicationName".to_string(),
            Value::String(name.to_string()),
        );
    }
    if let Some(title) = profile.get("identity.title") {
        product.insert("nameLong".to_string(), Value::String(title.to_string()));
    }
    if !bundle.is_empty() {
        product.insert(
            "darwinBundleIdentifier".to_string(),
            Value::String(bundle.to_string()),
        );
        // Deterministic, so an update keeps every installed copy's settings. See `derived_uuid`.
        for (name, purpose) in [
            ("win32AppUserModelId", "appusermodel"),
            ("win32RegValueName", "regvalue"),
            ("uuid", "product"),
        ] {
            product.insert(
                name.to_string(),
                Value::String(derived_uuid(bundle, purpose)),
            );
        }
    }
    if let Some(scheme) = profile.get("identity.url-scheme") {
        product.insert("urlProtocol".to_string(), Value::String(scheme.to_string()));
    }
    pretty(&Value::Object(product))
}

fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string());
    text.push('\n');
    text
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Astrid's own brand colours, read from `design/tokens.json`'s `brand` block.
///
/// Task 77367032's third point: that block is the single home of Astrid's defaults. The accent was
/// written in five places and they disagreed — the tokens said `#2563EB` while web's `config.ts`,
/// iOS's `Brand.swift` and Android's README still said `#3b82f6`. Reading it here rather than
/// restating it in the schema means this generator cannot become a sixth copy.
fn astrid_colours(root: &Path) -> BTreeMap<String, String> {
    let Ok(source) = std::fs::read_to_string(root.join(crate::design::SOURCE)) else {
        return BTreeMap::new();
    };
    let Ok(tokens) = serde_json::from_str::<Value>(&source) else {
        return BTreeMap::new();
    };
    let Some(brand) = tokens.get("brand").and_then(Value::as_object) else {
        return BTreeMap::new();
    };

    brand
        .iter()
        .filter_map(|(name, value)| {
            value
                .as_str()
                .map(|hex| (format!("colors.{name}"), hex.to_string()))
        })
        .collect()
}

fn load_schema(root: &Path) -> Result<Schema, ExitCode> {
    let path = root.join(SCHEMA);
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("cannot read {}: {error}", path.display());
            return Err(ExitCode::FAILURE);
        }
    };
    Schema::parse(&source).map_err(|problems| {
        eprintln!("{} is not a valid schema:", path.display());
        for problem in problems {
            eprintln!("  {problem}");
        }
        ExitCode::FAILURE
    })
}

/// `cargo xtask gen-brand <profile> --out <dir>`.
pub fn generate(root: &Path, profile_path: Option<String>, out: Option<String>) -> ExitCode {
    let (Some(profile_path), Some(out)) = (profile_path, out) else {
        eprintln!("usage: cargo xtask gen-brand <profile.brand.json> --out <dir>");
        return ExitCode::FAILURE;
    };

    let schema = match load_schema(root) {
        Ok(schema) => schema,
        Err(code) => return code,
    };

    let source = match std::fs::read_to_string(&profile_path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("cannot read {profile_path}: {error}");
            return ExitCode::FAILURE;
        }
    };

    let profile = match schema.read_with(&source, &astrid_colours(root)) {
        Ok(profile) => profile,
        Err(problems) => {
            eprintln!("{profile_path} does not match brand/schema.json:");
            for problem in problems {
                eprintln!("  {problem}");
            }
            return ExitCode::FAILURE;
        }
    };

    let out = Path::new(&out);
    for (relative, contents) in outputs(&schema, &profile) {
        let path = out.join(&relative);
        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                eprintln!("cannot create {}: {error}", parent.display());
                return ExitCode::FAILURE;
            }
        }
        if let Err(error) = std::fs::write(&path, contents) {
            eprintln!("cannot write {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
        println!("  {relative}");
    }

    println!(
        "{} brand resources written to {} (brand/schema.json v{})",
        profile.name,
        out.display(),
        schema.version()
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA_SOURCE: &str = r#"{
      "version": 1,
      "sections": {
        "identity": {
          "name": { "type": "string", "required": true, "env": "NEXT_PUBLIC_BRAND_NAME", "default": "Astrid" },
          "title": { "type": "string", "env": "NEXT_PUBLIC_BRAND_TITLE" },
          "domain": { "type": "domain", "required": true, "env": "NEXT_PUBLIC_BRAND_DOMAIN", "default": "astrid.cc" },
          "url-scheme": { "type": "scheme", "required": true, "env": "NEXT_PUBLIC_BRAND_APP_URL_SCHEME", "default": "astrid" },
          "support-email": { "type": "email", "env": "NEXT_PUBLIC_BRAND_SUPPORT_EMAIL" }
        },
        "colors": {
          "accent": { "type": "color", "required": true, "env": "NEXT_PUBLIC_BRAND_ACCENT_COLOR" },
          "accent-hover": { "type": "color", "derived": true },
          "accent-ink": { "type": "color", "derived": true },
          "accent-ink-dark": { "type": "color", "derived": true },
          "accent-text": { "type": "color", "derived": true }
        },
        "capabilities": {
          "mcp": { "type": "bool", "env": "NEXT_PUBLIC_BRAND_ENABLE_MCP" }
        }
      },
      "platforms": {
        "code": { "bundle-id": { "type": "string" } }
      }
    }"#;

    fn schema() -> Schema {
        Schema::parse(SCHEMA_SOURCE).expect("the test schema is valid")
    }

    /// The shape every profile in astrid-web's `brands/` has today: an `env` bag and nothing else.
    const ENV_ONLY: &str = r##"{
      "name": "Acme",
      "description": "A partner",
      "env": {
        "NEXT_PUBLIC_BRAND_NAME": "Acme",
        "NEXT_PUBLIC_BRAND_TITLE": "Acme Tasks",
        "NEXT_PUBLIC_BRAND_DOMAIN": "tasks.acme.example",
        "NEXT_PUBLIC_BRAND_APP_URL_SCHEME": "acmetasks",
        "NEXT_PUBLIC_BRAND_SUPPORT_EMAIL": "helpdesk@acme.example",
        "NEXT_PUBLIC_BRAND_ACCENT_COLOR": "#7e22ce",
        "NEXT_PUBLIC_BRAND_APP_STORE_URL": ""
      }
    }"##;

    /// The whole point of the aliases: nothing in astrid-web's `brands/` has to change for this
    /// to land, so the schema can go in before any client has migrated to the sections.
    #[test]
    fn todays_env_only_profile_validates_unchanged_77367032() {
        let profile = schema().read(ENV_ONLY).expect("valid");

        assert_eq!(profile.name, "Acme");
        assert_eq!(profile.get("identity.name"), Some("Acme"));
        assert_eq!(profile.get("identity.url-scheme"), Some("acmetasks"));
        assert_eq!(profile.get("colors.accent"), Some("#7e22ce"));
    }

    /// An empty string is how web's profiles spell "unset". It is absent, not invalid — otherwise
    /// every existing profile fails on `NEXT_PUBLIC_BRAND_APP_STORE_URL`.
    #[test]
    fn an_empty_optional_is_absent_not_invalid_77367032() {
        let profile = schema().read(ENV_ONLY).expect("valid");

        assert_eq!(profile.get("capabilities.mcp"), None);
    }

    /// A section wins over its alias, so a half-migrated profile is not quietly overridden by the
    /// key it has moved away from.
    #[test]
    fn a_section_beats_the_env_alias_77367032() {
        let source = r##"{
          "name": "Acme",
          "identity": { "name": "Acme", "domain": "new.example", "url-scheme": "acme" },
          "colors": { "accent": "#7E22CE" },
          "env": { "NEXT_PUBLIC_BRAND_DOMAIN": "old.example" }
        }"##;

        let profile = schema().read(source).expect("valid");

        assert_eq!(profile.get("identity.domain"), Some("new.example"));
    }

    /// The reason for a schema at all: a partner's broken profile says which field to fix.
    #[test]
    fn an_invalid_profile_names_the_field_77367032() {
        let source = r#"{
          "name": "Acme",
          "env": {
            "NEXT_PUBLIC_BRAND_NAME": "Acme",
            "NEXT_PUBLIC_BRAND_DOMAIN": "tasks.acme.example",
            "NEXT_PUBLIC_BRAND_APP_URL_SCHEME": "Acme Tasks",
            "NEXT_PUBLIC_BRAND_ACCENT_COLOR": "purple"
          }
        }"#;

        let problems = schema().read(source).expect_err("invalid");

        assert!(
            problems
                .iter()
                .any(|p| p.starts_with("colors.accent:") && p.contains("not a #rrggbb")),
            "{problems:?}"
        );
        assert!(
            problems
                .iter()
                .any(|p| p.starts_with("identity.url-scheme:")),
            "a scheme with a space never matches once the OS has it: {problems:?}"
        );
    }

    /// A missing required field names itself AND the key a profile may still be using for it.
    ///
    /// The accent, because it is the one required field with no Astrid default to fall back on:
    /// a partner's brand IS its accent, and quietly giving them Astrid's blue is the iOS bug this
    /// whole area exists to stop.
    #[test]
    fn a_missing_required_field_names_its_alias_77367032() {
        let source = r#"{ "name": "Acme", "env": { "NEXT_PUBLIC_BRAND_NAME": "Acme" } }"#;

        let problems = schema().read(source).expect_err("invalid");

        assert!(
            problems.contains(
                &"colors.accent is required (or env.NEXT_PUBLIC_BRAND_ACCENT_COLOR)".to_string()
            ),
            "{problems:?}"
        );
    }

    /// Astrid Code's ids must not move between runs: a regenerated GUID orphans every installed
    /// copy's settings. Same profile, same id — and a different bundle is a different id.
    #[test]
    fn derived_ids_are_stable_and_distinct_77367032() {
        let once = derived_uuid("example.acme.code", "product");
        let again = derived_uuid("example.acme.code", "product");
        let other = derived_uuid("example.other.code", "product");
        let purpose = derived_uuid("example.acme.code", "regvalue");

        assert_eq!(once, again, "the same profile must derive the same id");
        assert_ne!(once, other);
        assert_ne!(once, purpose);
        assert_eq!(once.len(), 36, "{once}");
        assert_eq!(once.as_bytes()[14], b'4', "the version nibble: {once}");
    }

    /// Every platform gets a file, from one profile. A client that silently produced nothing is
    /// the failure this task exists to stop.
    #[test]
    fn one_profile_writes_every_platform_77367032() {
        let schema = schema();
        let profile = schema.read(ENV_ONLY).expect("valid");
        let written = outputs(&schema, &profile);

        for expected in [
            "web/.env.brand",
            "web/brand.css",
            "apple/Brand.xcconfig",
            "apple/BrandInfo.plist",
            "android/brand.properties",
            "android/colors.xml",
            "windows/Brand.xaml",
            "windows/brand.manifest.json",
            "vscode/package.brand.json",
            "code/product.patch.json",
        ] {
            let found = written.iter().find(|(path, _)| path == expected);
            let (_, contents) = found.unwrap_or_else(|| panic!("{expected} was not written"));
            assert!(!contents.is_empty(), "{expected} is empty");
        }
    }

    /// The generated web environment is the same shape as the hand-written one, so a deployment
    /// cannot tell them apart — that is what makes the aliases a migration rather than a rewrite.
    #[test]
    fn the_web_environment_is_written_as_the_env_keys_77367032() {
        let schema = schema();
        let profile = schema.read(ENV_ONLY).expect("valid");
        let (_, env) = outputs(&schema, &profile)
            .into_iter()
            .find(|(path, _)| path == "web/.env.brand")
            .expect("written");

        assert!(env.contains("NEXT_PUBLIC_BRAND_NAME=Acme\n"), "{env}");
        assert!(
            env.contains("NEXT_PUBLIC_BRAND_ACCENT_COLOR=#7e22ce\n"),
            "{env}"
        );
        // Absent stays absent: an empty key would set it to "" in a real deployment.
        assert!(!env.contains("NEXT_PUBLIC_BRAND_APP_STORE_URL"), "{env}");
    }

    /// A partner states one accent and gets the rest, from the same rule every client shares
    /// (task 5ea48ab1). Before this, each client derived them differently and iOS fell back to
    /// Astrid's blue, so a purple brand had a blue hover on one platform out of four.
    #[test]
    fn one_accent_fills_the_colours_that_follow_from_it_5ea48ab1() {
        let profile = schema().read(ENV_ONLY).expect("valid");

        assert_eq!(profile.get("colors.accent"), Some("#7e22ce"));
        assert_eq!(profile.get("colors.accent-hover"), Some("#6B1DAF"));
        assert_eq!(profile.get("colors.accent-text"), Some("#FFFFFF"));
        assert!(profile.get("colors.accent-ink").is_some());
        assert!(profile.get("colors.accent-ink-dark").is_some());
    }

    /// An accent whose label cannot be read is refused by the generator too, not only by the
    /// rule — a partner finds out when they configure the brand rather than from a user.
    #[test]
    fn an_accent_that_cannot_carry_a_label_is_refused_5ea48ab1() {
        let source = r##"{
          "name": "Acme",
          "identity": { "name": "Acme", "domain": "a.example", "url-scheme": "acme" },
          "colors": { "accent": "#FFFF00" }
        }"##;

        let problems = schema().read(source).expect_err("refused");

        assert!(
            problems
                .iter()
                .any(|p| p.starts_with("colors.accent:") && p.contains("under the 4.5:1")),
            "{problems:?}"
        );
    }

    /// A stated value wins over a derived one: Astrid's own hover and inks are hand-picked on a
    /// designed ramp that no formula reproduces, and re-deriving them would overwrite the design.
    #[test]
    fn a_stated_colour_is_not_overwritten_by_derivation_5ea48ab1() {
        let source = r##"{
          "name": "Astrid",
          "identity": { "name": "Astrid", "domain": "a.example", "url-scheme": "astrid" },
          "colors": { "accent": "#2563EB", "accent-hover": "#1D4ED8" }
        }"##;

        let profile = schema().read(source).expect("valid");

        // Derivation would give #1F54C8 here; the designed next step on the ramp is #1D4ED8.
        assert_eq!(profile.get("colors.accent-hover"), Some("#1D4ED8"));
    }

    /// `astrid.brand.json` sets **nothing** — `"env": {}` — on purpose: it is the profile that
    /// proves the whitelabel refactor changed no behaviour. So a profile that states nothing must
    /// resolve to Astrid, and its colours must come from `design/tokens.json`'s `brand` block
    /// rather than from a sixth copy of the accent living in this generator.
    #[test]
    fn a_profile_that_states_nothing_is_astrid_77367032() {
        let defaults = BTreeMap::from([
            ("colors.accent".to_string(), "#2563EB".to_string()),
            ("colors.accent-hover".to_string(), "#1D4ED8".to_string()),
        ]);

        let profile = schema()
            .read_with(r#"{ "name": "Astrid", "env": {} }"#, &defaults)
            .expect("the empty profile is Astrid");

        assert_eq!(profile.get("identity.name"), Some("Astrid"));
        assert_eq!(profile.get("identity.domain"), Some("astrid.cc"));
        assert_eq!(profile.get("colors.accent"), Some("#2563EB"));
        // The tokens' hand-picked hover, NOT the derived #1F54C8 — the designed ramp wins.
        assert_eq!(profile.get("colors.accent-hover"), Some("#1D4ED8"));
    }

    /// The schema in the repository is the one the generator is written against, not a sample.
    #[test]
    fn the_shipped_schema_parses_77367032() {
        let source = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(SCHEMA),
        )
        .expect("brand/schema.json is in the repository");

        let schema = Schema::parse(&source).expect("the shipped schema is valid");

        assert_eq!(schema.version(), 1);
        for platform in ["web", "ios", "mac", "android", "windows", "vscode", "code"] {
            assert!(
                schema.platforms.contains_key(platform),
                "the schema must describe {platform}"
            );
        }
    }
}
