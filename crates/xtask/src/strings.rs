//! One string catalog, generated into every platform's format (task 1bdf84d7).
//!
//! `strings/` is the source: `catalog.json` says what each key interpolates, `locales.json` says
//! what the twelve languages are called on each platform, and `<locale>.json` holds the words.
//! `gen-strings` writes `strings/generated/`, and `check-strings` is the gate — exactly the shape
//! `gen-design` and `check-design` already have, for the same reason: a generated file that is
//! committed can be reviewed in a diff, and a check that regenerates and compares cannot be fooled
//! by a hand edit.
//!
//! ## Why the keys are respelled, and not the same way everywhere
//!
//! The core's keys are dotted, and they are dotted in a way that **cannot be nested**:
//! `filter.due` is a filter's own title AND the parent of `filter.due.today`. A JSON tree cannot
//! hold a string and an object at one path, next-intl resolves `t('filter.due.today')` by walking
//! the tree, and the Windows PRI compiler refuses a flat resource whose name is also a scope. So:
//!
//! | Platform | Key | Why |
//! |---|---|---|
//! | web | `filter_due_today` | next-intl walks `.` as a path, so a dotted key cannot be flat |
//! | Android | `filter_due_today` | a resource id is `[A-Za-z0-9_]` |
//! | Windows | `filter_due_today` | the PRI compiler refuses a name that is also a scope — and the shipped `Strings.Get` already does this exact swap |
//! | Apple | `filter.due.today` | `.strings` keys are opaque, so the core's own spelling survives |
//!
//! ## Why the arguments are positional everywhere but web
//!
//! The catalog is ICU with NAMED arguments — `Every {count} days`, `on the {ordinal} {weekday}`.
//! Three of the four formats have no names: `.resw` is `String.Format`'s `{0}`, Android is
//! `%1$s`, Apple is `%1$@`. So the catalog declares each key's arguments **in order**
//! (`catalog.json`), and that order is the contract. It is astrid-windows' `Converters.cs`, which
//! is the only shell assembling a repeat summary today — `repeat.on_month_and_day` is the month
//! and *then* the day, and swapping them would silently reword a shipped build.
//!
//! A language that reorders the words is exactly why the positional forms are *numbered*
//! (`%2$s` before `%1$s` is legal and correct); nothing here has to reorder anything.
//!
//! ## Plurals
//!
//! A locale's value may be an object of CLDR categories (`one`, `other`, …) instead of a string,
//! and `catalog.json` marks that key `"plural": true`. Every format below renders it natively —
//! ICU inline for web, `<plurals>` for Android, a `.stringsdict` for Apple, and one flat resource
//! per category for Windows, which has no plural resource at all.
//!
//! Five keys declare one (task 3564db06): the four `repeat.every_n_*` cadences and
//! `format.ordinal`. **Which** category a number selects is not this generator's business and is
//! not in `strings/` — it is `astrid_rules::plurals::category`, which is also what
//! [`problems`] checks a key's forms against.
//!
//! ## Ordinals
//!
//! `"ordinal": true` says those categories are CLDR's ordinal ones rather than its cardinal ones.
//! "3rd" is `few` in English and "3 weeks" is `other`, so a format that applied the wrong set
//! would write "3th" — which reads as a typo rather than as the wrong rule.
//!
//! Only web can select one natively (ICU `selectordinal`). The other three formats have no ordinal
//! resource at all, so an ordinal key reaches them as one flat resource per category and the client
//! picks. See [`KeySpec::ordinal`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::Value;

use crate::design::sha256;

/// Where the catalog lives, for the messages that tell somebody what to edit.
pub const SOURCE: &str = "strings";

/// One of the twelve languages, and what each platform calls it.
struct Locale {
    tag: String,
    name: String,
    web: String,
    apple: String,
    android: String,
    windows: String,
}

/// What `catalog.json` says about one key.
#[derive(Default)]
struct KeySpec {
    /// The arguments it interpolates, in the order every positional format numbers them.
    args: Vec<String>,
    /// Whether its value is an object of CLDR plural categories rather than a string.
    plural: bool,
    /// Whether those categories are the **ordinal** ones rather than the cardinal ones
    /// (task 3564db06). "3rd" is `few` in English and "3 weeks" is `other`; a format that applied
    /// the wrong set would write "3th" and look like a typo rather than a bug.
    ///
    /// Only web can select an ordinal natively (ICU `selectordinal`). Android's `<plurals>`, a
    /// `.stringsdict` and a `.resw` are all cardinal-only or flat, so an ordinal key goes to those
    /// three as one resource per category and the client asks
    /// `astrid_rules::plurals::category(locale, Ordinal, n)` which to use. That is not a
    /// shortcoming of this generator: those platforms have no ordinal plural at all.
    ordinal: bool,
}

/// One key's words in one language.
enum Message {
    Simple(String),
    /// CLDR category (`one`, `other`, …) to words. Kept sorted so output is stable.
    Plural(BTreeMap<String, String>),
}

impl Message {
    /// The form to use where a format cannot express a plural at all. `other` is the only category
    /// every language has.
    fn representative(&self) -> &str {
        match self {
            Message::Simple(text) => text,
            Message::Plural(forms) => forms
                .get("other")
                .or_else(|| forms.values().next())
                .map(String::as_str)
                .unwrap_or_default(),
        }
    }
}

/// The whole of `strings/`, read and checked for internal agreement.
struct Catalog {
    locales: Vec<Locale>,
    keys: BTreeMap<String, KeySpec>,
    /// Locale tag → key → words.
    messages: BTreeMap<String, BTreeMap<String, Message>>,
}

// ── Reading the source ──────────────────────────────────────────────────────────────────────────

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    serde_json::from_str(&text).map_err(|err| format!("{} is not JSON: {err}", path.display()))
}

fn string_field(object: &Value, field: &str, where_: &str) -> Result<String, String> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{where_} has no string `{field}`"))
}

fn read_catalog(root: &Path) -> Result<Catalog, String> {
    let dir = root.join(SOURCE);

    let locales_json = read_json(&dir.join("locales.json"))?;
    let rows = locales_json
        .get("locales")
        .and_then(Value::as_array)
        .ok_or("strings/locales.json has no `locales` array")?;
    let mut locales = Vec::new();
    for row in rows {
        let tag = string_field(row, "tag", "a row of strings/locales.json")?;
        let at = |field: &str| string_field(row, field, &format!("locale {tag}"));
        locales.push(Locale {
            name: at("name")?,
            web: at("web")?,
            apple: at("apple")?,
            android: at("android")?,
            windows: at("windows")?,
            tag,
        });
    }
    if locales.is_empty() {
        return Err("strings/locales.json names no locales".to_string());
    }

    let catalog_json = read_json(&dir.join("catalog.json"))?;
    let entries = catalog_json
        .get("keys")
        .and_then(Value::as_object)
        .ok_or("strings/catalog.json has no `keys` object")?;
    let mut keys: BTreeMap<String, KeySpec> = BTreeMap::new();
    for (key, spec) in entries {
        let args = match spec.get("args") {
            None => Vec::new(),
            Some(Value::Array(names)) => names
                .iter()
                .map(|name| {
                    name.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("{key}'s args must be strings"))
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err(format!("{key}'s `args` is not an array")),
        };
        keys.insert(
            key.clone(),
            KeySpec {
                args,
                plural: spec.get("plural").and_then(Value::as_bool).unwrap_or(false),
                ordinal: spec
                    .get("ordinal")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
        );
    }

    let mut messages = BTreeMap::new();
    for locale in &locales {
        let path = dir.join(format!("{}.json", locale.tag));
        let words = read_json(&path)?;
        let words = words
            .as_object()
            .ok_or_else(|| format!("{} is not an object of keys", path.display()))?;
        let mut table = BTreeMap::new();
        for (key, value) in words {
            let message = match value {
                Value::String(text) => Message::Simple(text.clone()),
                Value::Object(forms) => Message::Plural(
                    forms
                        .iter()
                        .map(|(category, text)| {
                            text.as_str()
                                .map(|text| (category.clone(), text.to_string()))
                                .ok_or_else(|| {
                                    format!("{key} in {} has a non-string form", locale.tag)
                                })
                        })
                        .collect::<Result<_, _>>()?,
                ),
                _ => {
                    return Err(format!(
                        "{key} in {} is neither words nor forms",
                        locale.tag
                    ))
                }
            };
            table.insert(key.clone(), message);
        }
        messages.insert(locale.tag.clone(), table);
    }

    Ok(Catalog {
        locales,
        keys,
        messages,
    })
}

// ── What the catalog has to say about itself ────────────────────────────────────────────────────

/// Everything wrong with the catalog, as sentences somebody can act on.
///
/// All of it at once rather than the first failure: a run that reports one missing key, is fixed,
/// and then reports the next is a translator's afternoon.
fn problems(catalog: &Catalog) -> Vec<String> {
    let mut problems = Vec::new();

    // 1. THE ACCEPTANCE: every key the core emits is in the catalog. This is the half that makes
    //    the catalog a contract rather than a file — a key added to `astrid-rules` and not here is
    //    a word no client can say, and it reaches the screen as the key itself.
    let emitted = astrid_rules::copy::keys();
    for key in &emitted {
        if !catalog.keys.contains_key(key) {
            problems.push(format!(
                "{key} is emitted by this core and is not in strings/catalog.json"
            ));
        }
    }
    // And nothing more: a key nobody emits is twelve translations that will never be read, and it
    // would sit there for ever because nothing would ever ask for it.
    for key in catalog.keys.keys() {
        if !emitted.contains(key) {
            problems.push(format!(
                "{key} is in strings/catalog.json but no longer emitted by this core"
            ));
        }
    }

    for locale in &catalog.locales {
        let table = &catalog.messages[&locale.tag];
        for (key, spec) in &catalog.keys {
            let Some(message) = table.get(key) else {
                problems.push(format!("{key} has no words in {}", locale.tag));
                continue;
            };

            // 2. A plural key is plural in every language or in none of them: the categories differ
            //    per language, but whether a key HAS categories is the key's own business.
            match (spec.plural, message) {
                (true, Message::Simple(_)) => problems.push(format!(
                    "{key} is marked plural but {} holds one string",
                    locale.tag
                )),
                (false, Message::Plural(_)) => problems.push(format!(
                    "{key} holds plural forms in {} but is not marked plural in catalog.json",
                    locale.tag
                )),
                _ => {}
            }
            if let Message::Plural(forms) = message {
                if !forms.contains_key("other") {
                    problems.push(format!(
                        "{key} in {} has no `other` form, which every language needs",
                        locale.tag
                    ));
                }
                // And every other form is a category something can ask for. A row spelled `One`
                // or `plural` is unreachable: `astrid_rules::plurals::category` never returns it,
                // so the lookup falls back to `other` and the language silently loses its rule.
                // A language is free to carry FEWER categories than its rule can return — the
                // Romance millions form spells the same words as `other` — but not other ones.
                for category in forms.keys() {
                    if !astrid_rules::plurals::CATEGORIES.contains(&category.as_str()) {
                        problems.push(format!(
                            "{key} in {} has a form called `{category}`, which is not a CLDR \
                             category — nothing will ever look it up",
                            locale.tag
                        ));
                    }
                }
            }

            // 3. Every language interpolates the same arguments. A translation that dropped
            //    `{count}` would render "Every days", and one that invented `{name}` would render
            //    the brace. Both are silent in every format.
            //
            //    An argument it invented is checked PER FORM, because any form carrying one is
            //    wrong. An argument it never uses is checked across the forms TOGETHER, because a
            //    plural's singular arm leaving the count out is not an omission, it is the point:
            //    English says "Every day", not "Every 1 day".
            let mut used: Vec<&str> = Vec::new();
            for form in forms_of(message) {
                for name in placeholders(form) {
                    if !spec.args.iter().any(|arg| arg == name) {
                        problems.push(format!(
                            "{key} in {} interpolates {{{name}}}, which catalog.json does not declare",
                            locale.tag
                        ));
                    }
                    if !used.contains(&name) {
                        used.push(name);
                    }
                }
            }
            for arg in &spec.args {
                if !used.iter().any(|name| name == arg) {
                    problems.push(format!(
                        "{key} in {} never uses {{{arg}}}, which catalog.json declares",
                        locale.tag
                    ));
                }
            }
        }
    }

    problems
}

/// Whether `provenance.json` still accounts for every key, and only for keys.
///
/// Its three lists have to partition the catalog. A key added by hand and not recorded there is a
/// translation whose evidence nobody can tell apart from a harvested one — which is the difference
/// between "a shipped translation" and "written by whoever added the key, wanting review".
fn provenance_problems(root: &Path, catalog: &Catalog) -> Vec<String> {
    let path = root.join(SOURCE).join("provenance.json");
    let Ok(value) = read_json(&path) else {
        return vec![format!("{} is missing or is not JSON", path.display())];
    };
    let mut recorded: Vec<String> = Vec::new();
    for list in ["harvested", "authored", "englishOnly"] {
        let Some(entries) = value.get(list).and_then(Value::as_array) else {
            return vec![format!("strings/provenance.json has no `{list}` array")];
        };
        recorded.extend(entries.iter().filter_map(Value::as_str).map(str::to_string));
    }

    let mut problems = Vec::new();
    for key in catalog.keys.keys() {
        if !recorded.contains(key) {
            problems.push(format!(
                "{key} is in the catalog but strings/provenance.json does not say where its \
                 translations came from"
            ));
        }
    }
    for key in &recorded {
        if !catalog.keys.contains_key(key) {
            problems.push(format!(
                "strings/provenance.json records {key}, which is not in the catalog"
            ));
        }
    }
    problems
}

fn forms_of(message: &Message) -> Vec<&str> {
    match message {
        Message::Simple(text) => vec![text.as_str()],
        Message::Plural(forms) => forms.values().map(String::as_str).collect(),
    }
}

/// The `{name}`s in a message, in the order they appear.
fn placeholders(message: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = message;
    while let Some(open) = rest.find('{') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('}') else { break };
        let name = &rest[..close];
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            found.push(name);
        }
        rest = &rest[close + 1..];
    }
    found
}

// ── Respelling a key and a message for one platform ─────────────────────────────────────────────

/// A dotted key as a flat resource name. The swap astrid-windows' `Strings.Get` already makes.
fn underscored(key: &str) -> String {
    key.replace('.', "_")
}

/// A message with its named arguments replaced by `render(index)`, numbering them by the order
/// `catalog.json` declares rather than the order this language happens to use them in.
fn positional(message: &str, args: &[String], render: impl Fn(usize) -> String) -> String {
    let mut out = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        rest = &rest[open + 1..];
        let Some(close) = rest.find('}') else {
            out.push('{');
            break;
        };
        let name = &rest[..close];
        match args.iter().position(|arg| arg == name) {
            Some(index) => out.push_str(&render(index)),
            // An unknown brace is left exactly as it was. `problems` has already reported it, and
            // mangling it here would make the report harder to act on.
            None => {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Android's own escaping on top of XML's: an apostrophe or a quote in the text ends the resource
/// unless it is backslashed, and a leading `@` or `?` is read as a reference.
fn escape_android(text: &str) -> String {
    let escaped = escape_xml(text).replace('\'', "\\'").replace('"', "\\\"");
    match escaped.chars().next() {
        Some('@') | Some('?') => format!("\\{escaped}"),
        _ => escaped,
    }
}

fn escape_strings_file(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

// ── The four formats ────────────────────────────────────────────────────────────────────────────

fn stamped(open: &str, close: &str, body: &str) -> String {
    format!(
        "{open} GENERATED by `cargo xtask gen-strings` from astrid-core strings/. Do not edit.{close}\n\
         {open} body-sha256: {}{close}\n{body}",
        sha256(body)
    )
}

/// next-intl messages: flat, underscored, ICU left exactly as the catalog wrote it.
fn web(catalog: &Catalog, locale: &Locale) -> String {
    let table = &catalog.messages[&locale.tag];
    let mut body = String::from("{\n");
    body.push_str(
        "  \"$generated\": \"by `cargo xtask gen-strings` from astrid-core strings/. Do not edit.\",\n",
    );
    let mut rows: Vec<String> = Vec::new();
    for (key, spec) in &catalog.keys {
        let Some(message) = table.get(key) else {
            continue;
        };
        let value = match message {
            Message::Simple(text) => text.clone(),
            // ICU's own plural syntax, which next-intl parses natively. The argument it switches
            // on is the first one declared, since that is the one a count can be.
            Message::Plural(forms) => {
                let on = spec.args.first().map(String::as_str).unwrap_or("count");
                let arms: Vec<String> = forms
                    .iter()
                    .map(|(category, text)| format!("{category} {{{text}}}"))
                    .collect();
                // `selectordinal` is a different rule set, not a different spelling: ICU's
                // `plural` would answer `other` for English's 3 and write "3th".
                let select = if spec.ordinal {
                    "selectordinal"
                } else {
                    "plural"
                };
                format!("{{{on}, {select}, {}}}", arms.join(" "))
            }
        };
        rows.push(format!(
            "  {}: {}",
            serde_json::to_string(&underscored(key)).unwrap_or_default(),
            serde_json::to_string(&value).unwrap_or_default()
        ));
    }
    body.push_str(&rows.join(",\n"));
    body.push_str("\n}\n");
    body
}

/// Apple's `Localizable.strings`: the core's dotted keys, positional `%n$@`.
fn apple_strings(catalog: &Catalog, locale: &Locale) -> String {
    let table = &catalog.messages[&locale.tag];
    let mut body = String::new();
    for (key, spec) in &catalog.keys {
        let Some(message) = table.get(key) else {
            continue;
        };
        let render = |index: usize| format!("%{}$@", index + 1);
        // An ORDINAL key has no home in the .stringsdict — Apple's only rule type is cardinal — so
        // its categories go here flat, `<key>.one` … `<key>.other`, and the client picks with
        // `astrid_rules::plurals::category`. The same shape Windows gets, for the same reason.
        if spec.ordinal {
            if let Message::Plural(forms) = message {
                for (category, text) in forms {
                    body.push_str(&format!(
                        "\"{}.{}\" = \"{}\";\n",
                        escape_strings_file(key),
                        escape_strings_file(category),
                        escape_strings_file(&positional(text, &spec.args, render))
                    ));
                }
                continue;
            }
        }
        // A cardinal plural lives in the .stringsdict beside this file; what goes here is the
        // fallback a build with no dictionary falls through to, which is the `other` form.
        let text = positional(message.representative(), &spec.args, render);
        body.push_str(&format!(
            "\"{}\" = \"{}\";\n",
            escape_strings_file(key),
            escape_strings_file(&text)
        ));
    }
    body
}

/// The `.stringsdict` beside it, written only when something is plural — an empty one is a file
/// for Xcode to ask questions about.
fn apple_stringsdict(catalog: &Catalog, locale: &Locale) -> Option<String> {
    let table = &catalog.messages[&locale.tag];
    let mut entries = String::new();
    for (key, spec) in &catalog.keys {
        let Some(Message::Plural(forms)) = table.get(key) else {
            continue;
        };
        // `NSStringPluralRuleType` is Apple's only rule type, and it is cardinal. An ordinal key
        // therefore cannot live here at all; `apple_strings` writes its categories flat instead.
        if spec.ordinal {
            continue;
        }
        let on = spec.args.first().map(String::as_str).unwrap_or("count");
        let index = spec.args.iter().position(|arg| arg == on).unwrap_or(0) + 1;
        entries.push_str(&format!(
            "\t<key>{}</key>\n\t<dict>\n\t\t<key>NSStringLocalizedFormatKey</key>\n\t\t<string>%#@{on}@</string>\n\t\t<key>{on}</key>\n\t\t<dict>\n\t\t\t<key>NSStringFormatSpecTypeKey</key>\n\t\t\t<string>NSStringPluralRuleType</string>\n\t\t\t<key>NSStringFormatValueTypeKey</key>\n\t\t\t<string>{index}$@</string>\n",
            escape_xml(key)
        ));
        for (category, text) in forms {
            let text = positional(text, &spec.args, |at| format!("%{}$@", at + 1));
            entries.push_str(&format!(
                "\t\t\t<key>{}</key>\n\t\t\t<string>{}</string>\n",
                escape_xml(category),
                escape_xml(&text)
            ));
        }
        entries.push_str("\t\t</dict>\n\t</dict>\n");
    }
    if entries.is_empty() {
        return None;
    }
    Some(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<!-- GENERATED by `cargo xtask gen-strings` from astrid-core strings/ ({}). Do not edit. -->\n<plist version=\"1.0\">\n<dict>\n{entries}</dict>\n</plist>\n",
        locale.tag
    ))
}

/// Android's `strings.xml`: underscored ids, `%n$s`, and real `<plurals>`.
fn android(catalog: &Catalog, locale: &Locale) -> String {
    let table = &catalog.messages[&locale.tag];
    let mut body = String::from("<resources>\n");
    for (key, spec) in &catalog.keys {
        let Some(message) = table.get(key) else {
            continue;
        };
        let name = underscored(key);
        let render = |index: usize| format!("%{}$s", index + 1);
        match message {
            Message::Simple(text) => body.push_str(&format!(
                "    <string name=\"{name}\">{}</string>\n",
                escape_android(&positional(text, &spec.args, render))
            )),
            // `<plurals>`' `quantity` is cardinal — Android has no ordinal resource — so an
            // ordinal key becomes one `<string>` per category and the client picks.
            Message::Plural(forms) if spec.ordinal => {
                for (category, text) in forms {
                    body.push_str(&format!(
                        "    <string name=\"{name}_{}\">{}</string>\n",
                        escape_xml(category),
                        escape_android(&positional(text, &spec.args, render))
                    ));
                }
            }
            Message::Plural(forms) => {
                body.push_str(&format!("    <plurals name=\"{name}\">\n"));
                for (category, text) in forms {
                    body.push_str(&format!(
                        "        <item quantity=\"{}\">{}</item>\n",
                        escape_xml(category),
                        escape_android(&positional(text, &spec.args, render))
                    ));
                }
                body.push_str("    </plurals>\n");
            }
        }
    }
    body.push_str("</resources>\n");
    body
}

/// A WinUI `.resw`: underscored names, `String.Format`'s `{0}`.
///
/// Windows has no plural resource, so a plural key becomes one resource per category — named
/// `<key>_one`, `<key>_other` — and the shell picks. That is what astrid-windows already does by
/// hand with its separate `repeat.every_day` and `repeat.every_n_days`.
fn windows(catalog: &Catalog, locale: &Locale) -> String {
    let table = &catalog.messages[&locale.tag];
    let mut body = String::new();
    let render = |index: usize| format!("{{{index}}}");
    let mut row = |name: &str, text: &str| {
        body.push_str(&format!(
            "  <data name=\"{}\" xml:space=\"preserve\">\n    <value>{}</value>\n  </data>\n",
            escape_xml(name),
            escape_xml(text)
        ));
    };
    for (key, spec) in &catalog.keys {
        let Some(message) = table.get(key) else {
            continue;
        };
        let name = underscored(key);
        match message {
            Message::Simple(text) => row(&name, &positional(text, &spec.args, render)),
            Message::Plural(forms) => {
                for (category, text) in forms {
                    row(
                        &format!("{name}_{category}"),
                        &positional(text, &spec.args, render),
                    );
                }
            }
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root>\n  <!--\n    GENERATED by `cargo xtask gen-strings` from astrid-core strings/ ({}, {}). Do not edit.\n\n    A resource name cannot contain a dot and a slash makes a scope, so the core's dotted keys\n    are stored flat with underscores — the swap Strings.Get already makes.\n  -->\n  <resheader name=\"resmimetype\"><value>text/microsoft-resx</value></resheader>\n  <resheader name=\"version\"><value>2.0</value></resheader>\n  <resheader name=\"reader\"><value>System.Resources.ResXResourceReader, System.Windows.Forms, Version=4.0.0.0, Culture=neutral, PublicKeyToken=b77a2c561934e089</value></resheader>\n  <resheader name=\"writer\"><value>System.Resources.ResXResourceWriter, System.Windows.Forms, Version=4.0.0.0, Culture=neutral, PublicKeyToken=b77a2c561934e089</value></resheader>\n{body}</root>\n",
        locale.tag, locale.name
    )
}

// ── Every file, and the two commands ────────────────────────────────────────────────────────────

/// Every generated file, with how many languages produced them — a count that is not
/// `files / per locale`, because the `.stringsdict` is written only where something is plural.
fn all_outputs(root: &Path) -> Result<(Vec<(String, String)>, usize), String> {
    let catalog = read_catalog(root)?;
    let mut found = problems(&catalog);
    found.extend(provenance_problems(root, &catalog));
    if !found.is_empty() {
        return Err(found.join("\n"));
    }

    let mut files = Vec::new();
    for locale in &catalog.locales {
        files.push((
            format!("{SOURCE}/generated/web/{}.json", locale.web),
            web(&catalog, locale),
        ));
        files.push((
            format!(
                "{SOURCE}/generated/apple/{}.lproj/Localizable.strings",
                locale.apple
            ),
            stamped("//", "", &apple_strings(&catalog, locale)),
        ));
        if let Some(dict) = apple_stringsdict(&catalog, locale) {
            files.push((
                format!(
                    "{SOURCE}/generated/apple/{}.lproj/Localizable.stringsdict",
                    locale.apple
                ),
                dict,
            ));
        }
        files.push((
            format!("{SOURCE}/generated/android/{}/strings.xml", locale.android),
            format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!-- GENERATED by `cargo xtask gen-strings` from astrid-core strings/ ({}). Do not edit. -->\n{}",
                locale.tag,
                android(&catalog, locale)
            ),
        ));
        files.push((
            format!(
                "{SOURCE}/generated/windows/{}/Resources.resw",
                locale.windows
            ),
            windows(&catalog, locale),
        ));
    }
    files.sort();
    Ok((files, catalog.locales.len()))
}

pub fn generate(root: &Path) -> ExitCode {
    let (files, _) = match all_outputs(root) {
        Ok(answer) => answer,
        Err(why) => {
            eprintln!("{why}");
            return ExitCode::FAILURE;
        }
    };
    for (relative, contents) in &files {
        let path = root.join(relative);
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
        println!("wrote  {relative}");
    }
    // Anything left behind from a locale or a format that has since gone is drift of the quietest
    // kind: it is committed, it looks generated, and nothing regenerates it.
    match orphans(root, &files) {
        Ok(orphans) if orphans.is_empty() => {}
        Ok(orphans) => {
            for relative in &orphans {
                let path = root.join(relative);
                if std::fs::remove_file(&path).is_ok() {
                    println!("removed {relative} (nothing generates it any more)");
                }
            }
        }
        Err(why) => println!("could not look for orphaned output: {why}"),
    }
    println!("\n{} file(s) from {SOURCE}/.", files.len());
    ExitCode::SUCCESS
}

pub fn check(root: &Path) -> ExitCode {
    let (files, languages) = match all_outputs(root) {
        Ok(answer) => answer,
        Err(why) => {
            eprintln!("{why}\n");
            eprintln!(
                "The catalog in {SOURCE}/ does not agree with this core. See strings/README.md."
            );
            return ExitCode::FAILURE;
        }
    };

    let mut stale = 0usize;
    for (relative, expected) in &files {
        match std::fs::read_to_string(root.join(relative)) {
            Ok(actual) if lf(&actual) == lf(expected) => println!("ok       {relative}"),
            Ok(_) => {
                println!("STALE    {relative}");
                stale += 1;
            }
            Err(_) => {
                println!("MISSING  {relative}");
                stale += 1;
            }
        }
    }
    match orphans(root, files.as_slice()) {
        Ok(orphans) => {
            for relative in &orphans {
                println!("ORPHANED {relative}");
                stale += 1;
            }
        }
        Err(why) => println!("could not look for orphaned output: {why}"),
    }

    if stale > 0 {
        eprintln!("\n{stale} generated file(s) are not what {SOURCE}/ says. Run `cargo xtask gen-strings`.");
        return ExitCode::FAILURE;
    }
    println!(
        "\n{} key(s) in {languages} language(s), generated into 4 formats.",
        astrid_rules::copy::keys().len(),
    );
    ExitCode::SUCCESS
}

/// Generated files on disk that `all_outputs` no longer names.
fn orphans(root: &Path, files: &[(String, String)]) -> Result<Vec<String>, String> {
    let generated = root.join(SOURCE).join("generated");
    if !generated.is_dir() {
        return Ok(Vec::new());
    }
    let expected: Vec<&str> = files
        .iter()
        .map(|(relative, _)| relative.as_str())
        .collect();
    let mut found = Vec::new();
    let mut stack = vec![generated.clone()];
    while let Some(directory) = stack.pop() {
        let entries = std::fs::read_dir(&directory)
            .map_err(|err| format!("cannot read {}: {err}", directory.display()))?;
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Some(relative) = relative_to(root, &path) else {
                continue;
            };
            if !expected.contains(&relative.as_str()) {
                found.push(relative);
            }
        }
    }
    found.sort();
    Ok(found)
}

fn relative_to(root: &Path, path: &Path) -> Option<String> {
    let relative: PathBuf = path.strip_prefix(root).ok()?.to_path_buf();
    Some(relative.to_string_lossy().replace('\\', "/"))
}

/// CRLF collapsed to LF. The same question `check-contracts` asks, for the same reason: a checkout
/// with `core.autocrlf=true` holds every generated file with CRLF while the committed bytes are
/// identical, and comparing those raw reports drift that is a `git config` setting (f1c7b53f).
fn lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(args: &[&str]) -> KeySpec {
        KeySpec {
            args: args.iter().map(|arg| arg.to_string()).collect(),
            plural: false,
            ordinal: false,
        }
    }

    /// Task 1bdf84d7. The named arguments the catalog carries are numbered by the order
    /// `catalog.json` declares, NOT by the order this language happens to use them — a language
    /// that puts the weekday before the ordinal still has to mark which is which, and every
    /// positional format numbers them for exactly that reason.
    #[test]
    fn arguments_are_numbered_by_the_declared_order_1bdf84d7() {
        let args = spec(&["ordinal", "weekday"]).args;
        let render = |index: usize| format!("%{}$s", index + 1);

        assert_eq!(
            positional("on the {ordinal} {weekday}", &args, render),
            "on the %1$s %2$s"
        );
        // Reordered by the translation, same numbers.
        assert_eq!(
            positional("{weekday} de la {ordinal} semana", &args, render),
            "%2$s de la %1$s semana"
        );
        assert_eq!(
            positional("on the {ordinal} {weekday}", &args, |index| format!(
                "{{{index}}}"
            )),
            "on the {0} {1}"
        );
    }

    /// An argument nothing declares is left exactly as written. `problems` has already said so by
    /// name, and a generator that silently ate the brace would make that report impossible to act
    /// on — the text on screen would look fine and the argument would simply never appear.
    #[test]
    fn an_undeclared_argument_is_left_alone_1bdf84d7() {
        assert_eq!(
            positional("Hello {nobody}", &spec(&["count"]).args, |index| format!(
                "{{{index}}}"
            )),
            "Hello {nobody}"
        );
    }

    /// The respelling, which is three different answers to one question. A key that is both a
    /// value and a namespace is the whole reason: `filter.due` and `filter.due.today` cannot both
    /// exist in a nested format, so three of the four flatten and Apple, which has no nesting,
    /// keeps the core's own spelling.
    #[test]
    fn a_dotted_key_flattens_for_the_formats_that_need_it_1bdf84d7() {
        assert_eq!(underscored("filter.due.today"), "filter_due_today");
        assert_eq!(underscored("sort"), "sort");
        assert_eq!(
            underscored("tasks.tradeoffRatings.veryGood"),
            "tasks_tradeoffRatings_veryGood"
        );
    }

    /// Android reads an unescaped apostrophe as the end of the resource and a leading `@` as a
    /// reference to another one. Both are silent: the build succeeds and the string is wrong.
    #[test]
    fn android_escapes_what_android_eats_1bdf84d7() {
        assert_eq!(escape_android("Won't do"), "Won\\'t do");
        assert_eq!(escape_android("@home"), "\\@home");
        assert_eq!(escape_android("a & b"), "a &amp; b");
    }

    #[test]
    fn the_placeholders_are_read_in_order_1bdf84d7() {
        assert_eq!(
            placeholders("on {month} the {ordinal}"),
            ["month", "ordinal"]
        );
        assert_eq!(placeholders("nothing here"), Vec::<&str>::new());
        // A brace that is not a placeholder is not one.
        assert_eq!(placeholders("{} {a b}"), Vec::<&str>::new());
    }

    /// A one-locale, one-key catalog, so the four writers can be asked about a single message.
    fn one_key(key: &str, spec: KeySpec, message: Message) -> Catalog {
        Catalog {
            locales: vec![Locale {
                tag: "en".into(),
                name: "English".into(),
                web: "en".into(),
                apple: "en".into(),
                android: "values".into(),
                windows: "en-US".into(),
            }],
            keys: BTreeMap::from([(key.to_string(), spec)]),
            messages: BTreeMap::from([(
                "en".to_string(),
                BTreeMap::from([(key.to_string(), message)]),
            )]),
        }
    }

    /// An ORDINAL key is a different rule set, and only one of the four formats can select it
    /// (task 3564db06).
    ///
    /// Web gets ICU's `selectordinal` — `plural` there would answer `other` for English's 3 and
    /// write "3th", which reads as a typo rather than as the wrong rule. The other three have no
    /// ordinal resource at all (`<plurals>`' `quantity` is cardinal, `NSStringPluralRuleType` is
    /// Apple's only rule type, and a `.resw` has nothing), so each gets one flat resource per
    /// category and the client asks `astrid_rules::plurals::category` which to read.
    #[test]
    fn an_ordinal_selects_by_the_ordinal_rules_3564db06() {
        let forms = BTreeMap::from([
            ("one".to_string(), "{count}st".to_string()),
            ("two".to_string(), "{count}nd".to_string()),
            ("few".to_string(), "{count}rd".to_string()),
            ("other".to_string(), "{count}th".to_string()),
        ]);
        let catalog = one_key(
            "format.ordinal",
            KeySpec {
                args: vec!["count".to_string()],
                plural: true,
                ordinal: true,
            },
            Message::Plural(forms),
        );
        let locale = &catalog.locales[0];

        let web = web(&catalog, locale);
        assert!(web.contains("selectordinal"), "{web}");
        assert!(!web.contains("count, plural"), "{web}");

        // Flat, one per category, on the three that cannot select an ordinal themselves.
        let android = android(&catalog, locale);
        assert!(android.contains("<string name=\"format_ordinal_few\">%1$srd</string>"));
        assert!(!android.contains("<plurals"), "{android}");
        assert!(apple_strings(&catalog, locale).contains("\"format.ordinal.few\" = \"%1$@rd\";"));
        assert!(
            apple_stringsdict(&catalog, locale).is_none(),
            "an ordinal has no home in a .stringsdict"
        );
        assert!(windows(&catalog, locale).contains("name=\"format_ordinal_few\""));

        // And it is still a legal catalog: the agreement checks have nothing to say about it.
        assert!(!problems(&catalog)
            .iter()
            .any(|problem| problem.contains("format.ordinal in en")));
    }

    /// A plural renders natively in every format rather than being flattened to its `other` arm.
    #[test]
    fn a_plural_renders_in_every_format_1bdf84d7() {
        let mut forms = BTreeMap::new();
        forms.insert("one".to_string(), "Every day".to_string());
        forms.insert("other".to_string(), "Every {count} days".to_string());
        let message = Message::Plural(forms);
        assert_eq!(message.representative(), "Every {count} days");

        let catalog = one_key(
            "repeat.every_n_days",
            KeySpec {
                args: vec!["count".to_string()],
                plural: true,
                ordinal: false,
            },
            message,
        );
        let locale = &catalog.locales[0];

        assert!(web(&catalog, locale)
            .contains("{count, plural, one {Every day} other {Every {count} days}}"));
        assert!(android(&catalog, locale).contains("<plurals name=\"repeat_every_n_days\">"));
        assert!(android(&catalog, locale).contains("<item quantity=\"one\">Every day</item>"));
        assert!(windows(&catalog, locale).contains("name=\"repeat_every_n_days_other\""));
        assert!(windows(&catalog, locale).contains("<value>Every {0} days</value>"));
        assert!(apple_strings(&catalog, locale)
            .contains("\"repeat.every_n_days\" = \"Every %1$@ days\";"));
        let dict = apple_stringsdict(&catalog, locale).expect("a plural needs a stringsdict");
        assert!(dict.contains("NSStringPluralRuleType"));
        assert!(dict.contains("<key>one</key>"));

        // And the agreement checks have nothing to say about the plural itself, which is what makes
        // the fixture a legal catalog rather than one this generator merely tolerates. (It is a
        // one-key catalog, so it is of course missing the other 105 keys this core emits; those
        // problems are the whole point of `problems` and are not what is being asserted here.)
        let about_the_key: Vec<String> = problems(&catalog)
            .into_iter()
            .filter(|problem| problem.starts_with("repeat.every_n_days"))
            .collect();
        assert!(
            about_the_key.is_empty(),
            "a plural whose singular arm omits the count is correct English, not a problem: \
             {about_the_key:?}"
        );
    }
}
